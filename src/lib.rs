//! Through lazy-pool you can create generic object pools which are lazily intitialized
//! and use a performant deque in order to provide instances of the pooled objects

//! The pool can be used in a threaded environment as well as an async environment
//! See Pool documentation for more info

use log::debug;
use std::{
    collections::VecDeque,
    fmt::{Debug, Error as FmtError, Formatter},
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

struct SharedPool<T> {
    size: usize,
    created: usize,
    pooled_items: VecDeque<T>,
    factory: Box<dyn Fn() -> T + Send + Sync>,
}

impl<T> Debug for SharedPool<T> {
    fn fmt(&self, fmt: &mut Formatter) -> Result<(), FmtError> {
        write!(
            fmt,
            "SharedPool(available: {}, total: {})",
            self.pooled_items.len(),
            self.created
        )
    }
}

#[derive(Debug)]
pub struct Pool<T> {
    shared_pool: Arc<Mutex<SharedPool<T>>>,
    tasks: Arc<Mutex<VecDeque<Waker>>>,
}

impl<T> Pool<T> {
    /// Default constructor for the Pool object:
    ///
    /// ```
    /// # extern crate lazy_pool;
    ///
    /// # use lazy_pool::Pool;
    ///
    /// # struct AnyObject;
    ///
    /// # fn main() {
    /// let pool = Pool::new(10, Box::new(|| AnyObject));
    /// # }
    /// ```
    ///
    /// The pool requires an object factory in order to be able to provide lazy
    /// initialization for objects.
    pub fn new(size: usize, factory: Box<dyn Fn() -> T + Send + Sync>) -> Self {
        Pool {
            tasks: Arc::new(Mutex::new(VecDeque::new())),
            shared_pool: Arc::new(Mutex::new(SharedPool {
                size,
                created: 0,
                pooled_items: VecDeque::new(),
                factory,
            })),
        }
    }

    /// To get an object out of the pool use get. This will return a future
    /// so you either need to await on it or to use it in an async manner
    ///
    /// ```
    /// # use futures::executor::block_on;
    /// # use lazy_pool::Pool;
    ///
    /// # struct AnyObject;
    ///
    /// # let pool = Pool::new(10, Box::new(|| AnyObject));
    /// let object = block_on(async { pool.get().await });
    /// ```
    pub fn get(&self) -> FuturePooled<T> {
        FuturePooled { pool: self.clone() }
    }

    fn release(&mut self, item: T) {
        let mut shared_pool = self.shared_pool.lock().unwrap();
        shared_pool.pooled_items.push_front(item);
        if let Some(task) = self.tasks.lock().unwrap().pop_front() {
            debug!("Notifying waiting task: {:?}", shared_pool);
            task.wake()
        }
        debug!("Releasing: {:?}", shared_pool);
    }

    fn get_if_available(&mut self) -> Option<T> {
        let mut shared_pool = self.shared_pool.lock().unwrap();
        match shared_pool.pooled_items.pop_front() {
            Some(object) => {
                debug!("Acquiring: {:?}", shared_pool);
                Some(object)
            }
            None => {
                if shared_pool.created < shared_pool.size {
                    shared_pool.created += 1;
                    debug!("Creating: {:?}", shared_pool);
                    Some((shared_pool.factory)())
                } else {
                    None
                }
            }
        }
    }
}

impl<T> Clone for Pool<T> {
    fn clone(&self) -> Self {
        Pool {
            shared_pool: self.shared_pool.clone(),
            tasks: self.tasks.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Pooled<T> {
    pool: Pool<T>,
    wrapped: Option<T>,
}

impl<T> Drop for Pooled<T> {
    fn drop(&mut self) {
        if let Some(item) = self.wrapped.take() {
            self.pool.release(item)
        }
    }
}

impl<T> DerefMut for Pooled<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.wrapped.as_mut().unwrap()
    }
}

impl<T> Deref for Pooled<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.wrapped.as_ref().unwrap()
    }
}

pub struct FuturePooled<T> {
    pool: Pool<T>,
}

impl<T> Future for FuturePooled<T> {
    type Output = Pooled<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        debug!("Polling pooled item");
        match self.pool.get_if_available() {
            Some(object) => {
                debug!("Pooled item ready!");
                Poll::Ready(Pooled {
                    pool: self.pool.clone(),
                    wrapped: Some(object),
                })
            }
            None => {
                self.pool
                    .tasks
                    .lock()
                    .unwrap()
                    .push_front(cx.waker().clone());
                debug!("Pooled item not Ready! Enqueueing task");
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {

    extern crate uuid;

    use super::*;

    use futures::executor::block_on;
    use std::{
        collections::HashSet,
        iter::FromIterator,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    #[derive(PartialEq, Debug)]
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

    #[test]
    fn can_get_object_from_pool() {
        let pool = Pool::new(
            10,
            Box::new(|| AnyObject {
                member: String::from("member"),
            }),
        );
        block_on(async {
            assert_eq!(
                AnyObject {
                    member: String::from("member")
                },
                *pool.get().await
            )
        })
    }

    #[test]
    fn can_get_multiple_objects_from_pool() {
        let pool = Pool::new(10, Box::new(AnyObject::new));
        block_on(async {
            assert!(*pool.get().await != *pool.get().await);
        })
    }

    #[test]
    fn item_is_relased_back_to_the_start_of_the_pool_when_dropped() {
        let pool = Pool::new(10, Box::new(AnyObject::new));
        block_on(async {
            let member_name_1 = (*pool.get().await).member.clone();
            let member_name_2 = (*pool.get().await).member.clone();
            let member_name_3 = (*pool.get().await).member.clone();
            assert_eq!(member_name_1, member_name_2);
            assert_eq!(member_name_2, member_name_3);
        })
    }

    #[test]
    fn can_share_pool_between_threads() {
        let pool = Pool::new(3, Box::new(|| AnyObject::new()));
        let members = Arc::new(Mutex::new(HashSet::<String>::new()));
        let mut handles = vec![];

        for _ in 1..10 {
            let local_pool = pool.clone();
            let local_members = members.clone();
            let handle = thread::spawn(move || {
                let pool = block_on(async { local_pool.get().await });
                let value = pool.member.clone();
                local_members.lock().unwrap().insert(value);
                thread::sleep(Duration::from_millis(200));
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(members.lock().unwrap().len(), 3);
    }

    #[test]
    fn can_use_closure_as_factory() {
        let context = "hello";
        let pool = Pool::new(10, Box::new(move || AnyObject::with_context(context)));
        block_on(async {
            assert_eq!(
                AnyObject {
                    member: String::from("hello")
                },
                *pool.get().await
            )
        })
    }

    struct AsyncPoolHolder {
        pool: Pool<AnyObject>,
    }

    impl AsyncPoolHolder {
        fn new() -> Self {
            Self {
                pool: Pool::new(3, Box::new(|| AnyObject::new())),
            }
        }

        fn get_member_value(self) -> Vec<String> {
            let mut values = vec![];
            block_on(async {
                let object_1 = self.pool.get().await;
                let object_2 = self.pool.get().await;
                let object_3 = self.pool.get().await;
                values.push(object_1.member.clone());
                values.push(object_2.member.clone());
                values.push(object_3.member.clone());
            });
            block_on(async {
                let object_4 = self.pool.get().await;
                let object_5 = self.pool.get().await;
                let object_6 = self.pool.get().await;
                values.push(object_4.member.clone());
                values.push(object_5.member.clone());
                values.push(object_6.member.clone());
            });
            values
        }
    }

    #[test]
    fn can_run_in_async() {
        let holder = AsyncPoolHolder::new();
        let res = holder.get_member_value();
        let set: HashSet<String> = HashSet::from_iter(res.iter().cloned());
        assert_eq!(3, set.len())
    }

    #[test]
    fn can_mutate_polled_items() {
        struct Mutable {
            count: i32,
        }

        let pool = Pool::new(2, Box::new(|| Mutable { count: 0 }));

        block_on(async {
            let mut object = pool.get().await;
            object.count += 1
        });
        block_on(async {
            let item_1 = pool.get().await;
            let item_2 = pool.get().await;

            assert_eq!(1, item_1.count);
            assert_eq!(0, item_2.count);
        })
    }
}
