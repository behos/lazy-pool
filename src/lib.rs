#![feature(box_syntax)]

use std::collections::LinkedList;
use std::ops::Deref;
use std::sync::{Arc, Mutex};


struct SharedPool<T> {
    size: usize,
    pooled_items: LinkedList<T>,
    factory: fn() -> T,
}


pub struct Pool<T> {
    shared_pool: Arc<Mutex<SharedPool<T>>>,
}


impl<T> Pool<T> {
    pub fn new(size: usize, factory: fn() -> T) -> Self {
        Pool {
            shared_pool: Arc::new(Mutex::new(SharedPool {
                size: size,
                pooled_items: LinkedList::new(),
                factory: factory,
            })),
        }
    }

    pub fn get(&mut self) -> Pooled<T> {
        let mut shared_pool = self.shared_pool.lock().unwrap();
        let item = shared_pool.pooled_items.pop_front().unwrap_or_else(
            shared_pool.factory,
        );
        Pooled {
            pool: self.clone(),
            wrapped: Some(item),
        }
    }

    fn release(&mut self, item: T) {
        (self.shared_pool.lock().unwrap()).pooled_items.push_front(
            item,
        )
    }
}


impl<T> Clone for Pool<T> {
    fn clone(&self) -> Self {
        Pool { shared_pool: self.shared_pool.clone() }
    }
}


pub struct Pooled<T> {
    pool: Pool<T>,
    wrapped: Option<T>,
}


impl<T> Drop for Pooled<T> {
    fn drop(&mut self) {
        self.pool.release(self.wrapped.take().unwrap())
    }
}

impl<T> Deref for Pooled<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.wrapped.as_ref().unwrap()
    }
}


#[cfg(test)]
mod tests {

    extern crate uuid;

    use std::thread;
    use std::collections::HashSet;
    use std::time::Duration;
    use std::sync::{Arc, Mutex};

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
        assert_eq!(AnyObject { member: String::from("member") }, *pool.get());
    }

    #[test]
    fn can_get_multiple_objects_from_pool() {
        let mut pool = Pool::new(10, AnyObject::new);
        assert!(*pool.get() != *pool.get());
    }

    #[test]
    fn can_item_is_relased_back_to_the_start_of_the_pool_when_dropped() {
        let mut pool = Pool::new(10, AnyObject::new);
        let member_name_1 = (*pool.get()).member.clone();
        let member_name_2 = (*pool.get()).member.clone();
        let member_name_3 = (*pool.get()).member.clone();
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
                let value = pool.member.clone();
                local_members.lock().unwrap().insert(value);
                thread::sleep(Duration::from_millis(100));
            });
            handles.push(handle);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(members.lock().unwrap().len() > 5);
    }
}
