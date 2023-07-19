//! Through lazy-pool you can create generic object pools which are lazily intitialized
//! and use a performant deque in order to provide instances of the pooled objects

//! The pool can be used in a threaded environment as well as an async environment
//! See Pool documentation for more info

mod error;
mod factory;

use error::LazyPoolError;
pub use factory::{Factory, SyncFactory};
use log::{debug, warn};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

pub use error::Result;

use futures::{channel::mpsc, lock::Mutex, select_biased, SinkExt, StreamExt};

#[macro_export]
macro_rules! get {
    ($item:ident = $pool:expr => $block:expr) => {{
        #[allow(unused_mut)]
        let mut $item = $pool.get().await;
        let res = $block;
        if let Err(err) = $item.release().await {
            ::log::error!("failed to release object: {err:?}");
        }
        res
    }};
}

pub struct Pool<T: Send> {
    factory: Arc<Mutex<Box<dyn Factory<T>>>>,
    return_receiver: Arc<Mutex<mpsc::Receiver<T>>>,
    create_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
    return_sender: mpsc::Sender<T>,
    create_sender: mpsc::Sender<()>,
}

impl<T: Send + 'static> Pool<T> {
    /**
    Default constructor for the Pool object:

    ```
    # extern crate lazy_pool;

    # use lazy_pool::Pool;
    # use futures::executor::block_on;

    # struct AnyObject;

    # fn main() {
    let pool = block_on(async { Pool::new(10, Box::new(|| AnyObject)).await.unwrap() });
    # }
    ```

    The pool requires an object factory in order to be able to provide lazy
    initialization for objects.
    Any synchronous closure can be used to create a SyncFactory.
    */
    pub async fn new<F>(size: usize, factory: F) -> Result<Self>
    where
        SyncFactory<T>: From<F>,
    {
        Self::new_with_factory(size, SyncFactory::from(factory)).await
    }

    /**
    Creating a Pool instance with a [`Factory`]
    Check [`Factory`] docs for an example of creating an async factory.
    */
    pub async fn new_with_factory<F>(size: usize, factory: F) -> Result<Self>
    where
        F: Factory<T> + 'static,
    {
        let (mut create_sender, create_receiver) = mpsc::channel(size);
        let (return_sender, return_receiver) = mpsc::channel(size);
        for _ in 0..size {
            create_sender.send(()).await?;
        }
        Ok(Pool {
            create_sender,
            return_sender,
            create_receiver: Arc::new(Mutex::new(create_receiver)),
            return_receiver: Arc::new(Mutex::new(return_receiver)),
            factory: Arc::new(Mutex::new(Box::new(factory))),
        })
    }

    /**
    To get an object out of the pool use get. This will return a future
    so you either need to await on it or to use it in an async manner

    ```
    # use futures::executor::block_on;
    # use lazy_pool::{Pool, Pooled, get};

    # struct AnyObject;


    let object = block_on(async {
        let pool = Pool::new(10, Box::new(|| AnyObject)).await.unwrap();
        get!(object = pool => {
            // Object was retrieved and assigned to `object`. It will be put back at
            // the end of this block, unless it's marked as tainted.
            Pooled::tainted(&mut object);
        });
    });
    ```
    */
    pub async fn get(&self) -> Pooled<T> {
        debug!("getting item");
        let object = self.next_available().await;
        Pooled {
            wrapped: Some(object),
            tainted: false,
            create_sender: self.create_sender.clone(),
            return_sender: self.return_sender.clone(),
        }
    }

    async fn next_available(&self) -> T {
        let mut return_receiver = self.return_receiver.lock().await;
        let mut create_receiver = self.create_receiver.lock().await;
        select_biased! {
            item = return_receiver.next() => {
                debug!("using returned object");
                item.expect("whoops")
            },
            _ = create_receiver.next() => {
                debug!("creating object");
                self.create().await
            }
        }
    }

    async fn create(&self) -> T {
        self.factory.lock().await.produce().await
    }
}

pub struct Pooled<T: Send + 'static> {
    wrapped: Option<T>,
    tainted: bool,
    return_sender: mpsc::Sender<T>,
    create_sender: mpsc::Sender<()>,
}

impl<T: Send> Pooled<T> {
    pub fn tainted(&mut self) {
        self.tainted = true;
    }

    pub async fn release(mut self) -> Result<()> {
        debug!("releasing object (tainted = {})", self.tainted);
        match (self.tainted, self.wrapped.take()) {
            (_, None) => {
                warn!("release called multiple times");
                Ok(())
            }
            (true, _) => self.create_sender.send(()).await,
            (false, Some(item)) => self.return_sender.send(item).await,
        }
        .map_err(|_| LazyPoolError::Release)
    }
}

impl<T: Send> DerefMut for Pooled<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.wrapped.as_mut().unwrap()
    }
}

impl<T: Send> Deref for Pooled<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.wrapped.as_ref().unwrap()
    }
}

#[cfg(test)]
mod tests {

    extern crate uuid;

    use super::*;

    use futures::{executor::block_on, select, Future, FutureExt};
    use futures_timer::Delay;
    use log::debug;
    use std::{
        collections::HashSet,
        iter::FromIterator,
        sync::{Arc, Mutex as SyncMutex},
        thread,
        time::Duration,
    };
    use test_log::test;
    use tokio::task::JoinSet;

    #[derive(PartialEq, Debug, Clone)]
    struct AnyObject {
        member: String,
    }

    impl AnyObject {
        fn new() -> Self {
            AnyObject {
                member: uuid::Uuid::new_v4().to_string(),
            }
        }

        fn with_context(context: &str) -> Self {
            AnyObject {
                member: String::from(context),
            }
        }
    }

    #[test(tokio::test)]
    async fn can_get_object_from_pool() {
        let pool = Pool::new(
            10,
            Box::new(|| AnyObject {
                member: String::from("member"),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            AnyObject {
                member: String::from("member")
            },
            get!(item = pool => (*item).clone())
        )
    }

    #[test(tokio::test)]
    async fn can_get_multiple_objects_from_pool() {
        let pool = Pool::new(10, Box::new(AnyObject::new)).await.unwrap();
        get!(item_1 = pool => {
            get!(item_2 = pool => {
                assert!(*item_1 != *item_2);
            })
        })
    }

    #[test(tokio::test)]
    async fn item_is_relased_back_to_the_start_of_the_pool_when_dropped() {
        let pool = Pool::new(10, Box::new(AnyObject::new)).await.unwrap();
        let member_name_1 = get!(item = pool => item.member.clone());
        let member_name_2 = get!(item = pool => item.member.clone());
        let member_name_3 = get!(item = pool => item.member.clone());
        assert_eq!(member_name_1, member_name_2);
        assert_eq!(member_name_2, member_name_3);
    }

    #[test(tokio::test)]
    async fn pool_does_not_go_over_capacity_when_running_out_of_items() {
        let pool = Pool::new(2, Box::new(AnyObject::new)).await.unwrap();
        get!(item_1 = pool => {
            get!(item_2 = pool => {
                let mut timeout = Delay::new(Duration::from_millis(100)).fuse();
                select! {
                    _ = pool.get().fuse() => panic!("should not be able to get this"),
                    _ = timeout => {}
                }
            });
            let mut timeout = Delay::new(Duration::from_millis(100)).fuse();
            select! {
                _ = pool.get().fuse() => {},
                _ = timeout => panic!("should be able to get this"),
            }
        });
    }

    #[test]
    fn can_share_pool_between_threads_in_sync_code() {
        let pool = Arc::new(block_on(async {
            Pool::new(3, Box::new(|| AnyObject::new())).await.unwrap()
        }));
        let members = Arc::new(SyncMutex::new(HashSet::<String>::new()));
        let mut handles = vec![];

        for _ in 1..10 {
            let local_pool = pool.clone();
            let local_members = members.clone();
            let handle = thread::spawn(move || {
                let value = block_on(async {
                    get!(item = local_pool => {
                        Delay::new(Duration::from_millis(100)).await;
                        item.member.clone()
                    })
                });
                debug!("adding value to members");
                local_members.lock().unwrap().insert(value);
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(members.lock().unwrap().len(), 3);
    }

    #[test(tokio::test)]
    async fn can_use_closure_as_factory() {
        let context = "hello";
        let pool = Pool::new(10, Box::new(move || AnyObject::with_context(context)))
            .await
            .unwrap();
        assert_eq!(
            AnyObject {
                member: String::from("hello")
            },
            get!(item = pool => item.clone())
        )
    }

    struct AsyncFactory {}

    impl AsyncFactory {
        async fn get_instance(&self) -> AnyObject {
            AnyObject {
                member: String::from("hello"),
            }
        }
    }

    impl Factory<AnyObject> for AsyncFactory {
        fn produce(&mut self) -> Box<dyn Future<Output = AnyObject> + Send + Unpin + '_> {
            Box::new(Box::pin(self.get_instance()))
        }
    }

    #[test(tokio::test)]
    async fn can_use_async_function_as_factory() {
        let pool = Pool::new_with_factory(10, AsyncFactory {}).await.unwrap();
        assert_eq!(
            AnyObject {
                member: String::from("hello")
            },
            get!(item = pool => item.clone())
        )
    }

    struct AsyncPoolHolder {
        pool: Pool<AnyObject>,
    }

    impl AsyncPoolHolder {
        async fn new() -> Self {
            Self {
                pool: Pool::new(3, Box::new(|| AnyObject::new())).await.unwrap(),
            }
        }

        async fn get_member_value(self) -> Vec<String> {
            let mut values = vec![];
            get!(object_1 = self.pool => {
                get!(object_2 = self.pool => {
                    get!(object_3 = self.pool => {
                        values.push(object_1.member.clone());
                        values.push(object_2.member.clone());
                        values.push(object_3.member.clone());
                    })
                })
            });
            get!(object_4 = self.pool => {
                get!(object_5 = self.pool => {
                    get!(object_6 = self.pool => {
                        values.push(object_4.member.clone());
                        values.push(object_5.member.clone());
                        values.push(object_6.member.clone());
                    })
                })
            });
            values
        }
    }

    #[test(tokio::test)]
    async fn can_run_in_async() {
        let holder = AsyncPoolHolder::new().await;
        let res = holder.get_member_value().await;
        let set: HashSet<String> = HashSet::from_iter(res.iter().cloned());
        assert_eq!(3, set.len())
    }

    #[test(tokio::test)]
    async fn can_mutate_polled_items() {
        struct Mutable {
            count: i32,
        }

        let pool = Pool::new(2, Box::new(|| Mutable { count: 0 }))
            .await
            .unwrap();

        get!(object = pool => {
            object.count += 1;
        });

        get!(item_1 = pool => {
            get!(item_2 = pool => {
                assert_eq!(1, item_1.count);
                assert_eq!(0, item_2.count);
            })
        })
    }

    #[test(tokio::test)]
    async fn marking_item_as_tainted_drops_it_from_pool() {
        let pool = Pool::new(1, Box::new(AnyObject::new)).await.unwrap();

        let mut item_val = String::new();
        get!(item = pool => {
            item_val += &item.member;
        });
        get!(item = pool => {
            assert!(item_val == item.member);
            Pooled::tainted(&mut item);
        });
        get!(item = pool => {
            assert!(item_val != item.member);
        });
    }

    #[test(tokio::test)]
    async fn can_run_in_multi_task_mode() {
        let pool = Arc::new(Pool::new(1, Box::new(AnyObject::new)).await.unwrap());
        let mut join_set = JoinSet::new();
        for _ in 0..10 {
            let local_pool = pool.clone();
            join_set.spawn(async move {
                get!(item = local_pool => {
                    println!("{}", item.member);
                });
            });
        }
        while !join_set.is_empty() {
            join_set.join_next().await;
        }
    }
}
