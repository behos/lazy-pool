#![feature(box_syntax)]

extern crate futures;
pub mod errors;

use std::collections::LinkedList;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use futures::{Future, Async, Poll};


use errors::PoolError;


#[derive(Debug)]
struct SharedPool<T> {
    size: usize,
    created: usize,
    pooled_items: LinkedList<T>,
    factory: fn() -> T,
}


#[derive(Debug)]
pub struct Pool<T> {
    shared_pool: Arc<Mutex<SharedPool<T>>>,
}


impl<T> Pool<T> {
    pub fn new(size: usize, factory: fn() -> T) -> Self {
        Pool {
            shared_pool: Arc::new(Mutex::new(SharedPool {
                size: size,
                created: 0,
                pooled_items: LinkedList::new(),
                factory: factory,
            })),
        }
    }

    pub fn get(&mut self) -> FuturePooled<T> {
        FuturePooled {
            pool: self.clone(),
            taken: false,
        }
    }

    fn release(&mut self, item: T) {
        (self.shared_pool.lock().unwrap()).pooled_items.push_front(
            item,
        )
    }

    fn get_if_available(&mut self) -> Option<T> {
        let mut shared_pool = self.shared_pool.lock().unwrap();
        match shared_pool.pooled_items.pop_front() {
            Some(object) => Some(object),
            None => {
                if shared_pool.created < shared_pool.size {
                    shared_pool.created += 1;
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
        Pool { shared_pool: self.shared_pool.clone() }
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
        if self.taken {
            Err(PoolError::PollError)
        } else {
            match self.pool.get_if_available() {
                Some(object) => {
                    self.taken = true;
                    Ok(Async::Ready(Pooled {
                        pool: self.pool.clone(),
                        wrapped: Some(object),
                    }))
                }
                None => Ok(Async::NotReady),
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

    use futures::{Future, Async};

    use super::Pool;

    #[derive(PartialEq, Debug)]
    struct AnyObject {
        member: String,
    }

    impl AnyObject {
        fn new() -> Self {
            AnyObject { member: uuid::Uuid::new_v4().to_string() }
        }
    }

    #[test]
    fn can_get_object_from_pool() {
        let mut pool = Pool::new(10, || AnyObject { member: String::from("member") });
        assert_eq!(
            AnyObject { member: String::from("member") },
            *pool.get().wait().unwrap()
        );
    }

    #[test]
    fn can_get_multiple_objects_from_pool() {
        let mut pool = Pool::new(10, AnyObject::new);
        assert!(*pool.get().wait().unwrap() != *pool.get().wait().unwrap());
    }

    #[test]
    fn item_is_relased_back_to_the_start_of_the_pool_when_dropped() {
        let mut pool = Pool::new(10, AnyObject::new);
        let member_name_1 = (*pool.get().wait().unwrap()).member.clone();
        let member_name_2 = (*pool.get().wait().unwrap()).member.clone();
        let member_name_3 = (*pool.get().wait().unwrap()).member.clone();
        assert_eq!(member_name_1, member_name_2);
        assert_eq!(member_name_2, member_name_3);
    }

    #[test]
    fn can_share_pool_between_threads() {
        let pool = Pool::new(10, AnyObject::new);
        let members = Arc::new(Mutex::new(HashSet::<String>::new()));
        let mut handles = vec![];

        for _ in 1..10 {
            let mut local_pool = pool.clone();
            let local_members = members.clone();
            let handle = thread::spawn(move || {
                let pool = local_pool.get();
                let value = pool.wait().unwrap().member.clone();
                local_members.lock().unwrap().insert(value);
                thread::sleep(Duration::from_millis(200));
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(members.lock().unwrap().len() > 3);
    }

    #[test]
    fn additional_polls_need_to_wait_for_the_pooled_item_to_be_freed() {
        let mut pool = Pool::new(1, AnyObject::new);
        let mut first = pool.get();
        let mut second = pool.get();

        {
            // While the item is in scope, it's busy
            let item = match first.poll() {
                Ok(Async::NotReady) => None,
                Ok(Async::Ready(o)) => Some(o),
                _ => None,
            };

            assert!(item.is_some());

            match second.poll() {
                Ok(Async::Ready(_)) => assert!(false),
                _ => (),
            }
        }

        let item = match second.poll() {
            Ok(Async::NotReady) => None,
            Ok(Async::Ready(o)) => Some(o),
            _ => None,
        };
        assert!(item.is_some());
    }

    #[test]
    fn polls_on_consumed_future_raise_error() {
        let mut pool = Pool::new(1, AnyObject::new);
        let mut first = pool.get();
        match first.poll() {
            Ok(Async::NotReady) => None,
            Ok(Async::Ready(o)) => Some(o),
            _ => None,
        };
        let poll_result = first.poll();
        assert!(poll_result.is_err());
    }

}
