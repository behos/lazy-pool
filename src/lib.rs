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

    use super::Pool;

    #[derive(PartialEq, Debug)]
    struct AnyObject {
        member: i32,
    }

    #[test]
    fn can_get_object_from_pool() {

        let mut pool = Pool::new(10, || AnyObject { member: 4 });
        assert_eq!(AnyObject { member: 4 }, *pool.get());
    }
}
