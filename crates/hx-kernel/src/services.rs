use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

/// A registry of shared services keyed by type.
///
/// The key is usually a trait object type, so a provider can be swapped
/// without its consumers changing: `services.insert::<dyn Sandbox>(arc)`.
#[derive(Default)]
pub struct Services {
    map: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

/// Returned by [`Services::require`] when nothing is registered for a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingService(pub &'static str);

impl fmt::Display for MissingService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no service registered for `{}`", self.0)
    }
}

impl std::error::Error for MissingService {}

impl Services {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `service`, returning the one it replaced.
    pub fn insert<T>(&self, service: Arc<T>) -> Option<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let previous = self
            .map
            .write()
            .expect("services lock poisoned")
            .insert(TypeId::of::<T>(), Box::new(service));
        previous.and_then(|boxed| boxed.downcast::<Arc<T>>().ok().map(|arc| *arc))
    }

    pub fn remove<T>(&self) -> Option<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let previous = self
            .map
            .write()
            .expect("services lock poisoned")
            .remove(&TypeId::of::<T>());
        previous.and_then(|boxed| boxed.downcast::<Arc<T>>().ok().map(|arc| *arc))
    }

    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.map
            .read()
            .expect("services lock poisoned")
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<Arc<T>>())
            .cloned()
    }

    pub fn require<T>(&self) -> Result<Arc<T>, MissingService>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.get::<T>().ok_or(MissingService(type_name::<T>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Greeter: Send + Sync {
        fn greet(&self) -> String;
    }

    struct English;
    impl Greeter for English {
        fn greet(&self) -> String {
            "hello".into()
        }
    }

    struct Vietnamese;
    impl Greeter for Vietnamese {
        fn greet(&self) -> String {
            "xin chào".into()
        }
    }

    #[test]
    fn providers_are_swappable_behind_a_trait_key() {
        let services = Services::new();
        assert_eq!(
            services.require::<dyn Greeter>().err(),
            Some(MissingService(type_name::<dyn Greeter>()))
        );

        services.insert::<dyn Greeter>(Arc::new(English));
        assert_eq!(services.require::<dyn Greeter>().unwrap().greet(), "hello");

        let replaced = services.insert::<dyn Greeter>(Arc::new(Vietnamese));
        assert_eq!(replaced.unwrap().greet(), "hello");
        assert_eq!(services.get::<dyn Greeter>().unwrap().greet(), "xin chào");

        assert!(services.remove::<dyn Greeter>().is_some());
        assert!(services.get::<dyn Greeter>().is_none());
    }
}
