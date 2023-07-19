use std::future::{ready, Future};

/** The factory trait is used to populate the Pool when items are
created and replaced. There is a default implementation of factory
for boxed sync closures, and creating an asynchronous factory is
shown in this example:

```
use std::future::Future;
use lazy_pool::Factory;

struct AsyncFactory {}

struct AnyObject {
    member: String,
}

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
````
*/
pub trait Factory<T>: Send
where
    T: Send,
{
    fn produce(&mut self) -> Box<dyn Future<Output = T> + Unpin + Send + '_>;
}

pub struct SyncFactory<T> {
    func: Box<dyn Fn() -> T + Send + Sync>,
}

impl<T> Factory<T> for SyncFactory<T>
where
    T: Send + 'static,
{
    fn produce(&mut self) -> Box<dyn Future<Output = T> + Unpin + Send + '_> {
        Box::new(ready((self.func)()))
    }
}

impl<C, T> From<C> for SyncFactory<T>
where
    C: Fn() -> T + Send + Sync + 'static,
{
    fn from(func: C) -> Self {
        Self {
            func: Box::new(func),
        }
    }
}
