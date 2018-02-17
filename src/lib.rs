#![feature(box_syntax, universal_impl_trait)]

#[macro_use]
extern crate log;
extern crate futures;

pub mod errors;

use std::collections::LinkedList;
use std::fmt::{Debug, Formatter, Error as FmtError};
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use futures::{Future, Async, Poll};
use futures::task::{current, Task};

use errors::PoolError;


struct SharedPool<T> {
    size: usize,
    created: usize,
    pooled_items: LinkedList<T>,
    factory: Box<Fn() -> T + Send + Sync>,
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
    tasks: Arc<Mutex<LinkedList<Task>>>,
}


impl<T> Pool<T> {
    pub fn new(size: usize, factory: Box<Fn() -> T + Send + Sync>) -> Self {
        Pool {
            tasks: Arc::new(Mutex::new(LinkedList::new())),
            shared_pool: Arc::new(Mutex::new(SharedPool {
                size: size,
                created: 0,
                pooled_items: LinkedList::new(),
                factory: factory,
            })),
        }
    }

    pub fn get(&self) -> FuturePooled<T> {
        FuturePooled {
            pool: self.clone(),
            taken: false,
        }
    }

    fn release(&mut self, item: T) {
        let mut shared_pool = self.shared_pool.lock().unwrap();
        shared_pool.pooled_items.push_front(item);
        if let Some(task) = self.tasks.lock().unwrap().pop_front() {
            debug!("Notifying waiting task: {:?}", shared_pool);
            task.notify()
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
        match self.wrapped.take() {
            Some(item) => self.pool.release(item),
            None => (),
        }
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
    taken: bool,
}


impl<T> Future for FuturePooled<T> {
    type Item = Pooled<T>;
    type Error = PoolError;

    fn poll<'a>(&'a mut self) -> Poll<Self::Item, Self::Error> {
        debug!("Polling pooled item");
        if self.taken {
            Err(PoolError::PollError)
        } else {
            match self.pool.get_if_available() {
                Some(object) => {
                    self.taken = true;
                    debug!("Pooled item ready!");
                    Ok(Async::Ready(Pooled {
                        pool: self.pool.clone(),
                        wrapped: Some(object),
                    }))
                }
                None => {
                    self.pool.tasks.lock().unwrap().push_front(current());
                    debug!("Pooled item not Ready! Enqueueing task");
                    Ok(Async::NotReady)
                }
            }
        }
    }
}


#[cfg(test)]
mod tests {

    extern crate uuid;

    use std::thread;
    use std::collections::HashSet;
    use std::time::Duration;
    use std::sync::{Arc, Mutex};

    use futures::Future;

    use super::Pool;

    #[derive(PartialEq, Debug)]
    struct AnyObject {
        member: String,
    }

    impl AnyObject {
        fn new() -> Self {
            AnyObject { member: uuid::Uuid::new_v4().to_string() }
        }

        fn with_context(context: &str) -> Self {
            AnyObject { member: String::from(context) }
        }
    }

    #[test]
    fn can_get_object_from_pool() {
        let pool = Pool::new(10, box || AnyObject { member: String::from("member") });
        assert_eq!(
            AnyObject { member: String::from("member") },
            *pool.get().wait().unwrap()
        );
    }

    #[test]
    fn can_get_multiple_objects_from_pool() {
        let pool = Pool::new(10, box AnyObject::new);
        assert!(*pool.get().wait().unwrap() != *pool.get().wait().unwrap());
    }

    #[test]
    fn item_is_relased_back_to_the_start_of_the_pool_when_dropped() {
        let pool = Pool::new(10, box AnyObject::new);
        let member_name_1 = (*pool.get().wait().unwrap()).member.clone();
        let member_name_2 = (*pool.get().wait().unwrap()).member.clone();
        let member_name_3 = (*pool.get().wait().unwrap()).member.clone();
        assert_eq!(member_name_1, member_name_2);
        assert_eq!(member_name_2, member_name_3);
    }

    #[test]
    fn can_share_pool_between_threads() {
        let pool = Pool::new(3, box || AnyObject::new());
        let members = Arc::new(Mutex::new(HashSet::<String>::new()));
        let mut handles = vec![];

        for _ in 1..10 {
            let mut local_pool = pool.clone();
            let local_members = members.clone();
            let handle = thread::spawn(move || {
                let pool = local_pool.get().wait().unwrap();
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
        let pool = Pool::new(10, box move || AnyObject::with_context(context));
        assert_eq!(
            AnyObject { member: String::from("hello") },
            *pool.get().wait().unwrap()
        );
    }
}
