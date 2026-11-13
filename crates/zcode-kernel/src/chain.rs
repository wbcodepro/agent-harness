use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::Registration;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One layer of around-middleware.
///
/// A layer receives the request and the rest of the chain. It delegates with
/// `next.run(req)`, or returns without calling it to short-circuit.
pub trait Layer<Req, Res>: Send + Sync + 'static {
    fn call<'a>(&'a self, req: Req, next: Next<'a, Req, Res>) -> BoxFuture<'a, Res>;
}

type Layers<Req, Res> = Vec<(u64, Arc<dyn Layer<Req, Res>>)>;
type Terminal<'a, Req, Res> = dyn Fn(Req) -> BoxFuture<'a, Res> + Send + Sync + 'a;

/// The remainder of a chain: the layers after the current one, then the
/// terminal operation.
pub struct Next<'a, Req, Res> {
    rest: &'a [Arc<dyn Layer<Req, Res>>],
    terminal: &'a Terminal<'a, Req, Res>,
}

impl<'a, Req: 'static, Res: 'static> Next<'a, Req, Res> {
    pub fn run(self, req: Req) -> BoxFuture<'a, Res> {
        match self.rest.split_first() {
            Some((head, rest)) => head.call(
                req,
                Next {
                    rest,
                    terminal: self.terminal,
                },
            ),
            None => (self.terminal)(req),
        }
    }
}

/// An ordered set of layers wrapped around a terminal operation. The first
/// layer is outermost.
pub struct Chain<Req, Res> {
    layers: RwLock<Layers<Req, Res>>,
    next_id: AtomicU64,
}

impl<Req, Res> Default for Chain<Req, Res> {
    fn default() -> Self {
        Self {
            layers: RwLock::new(Vec::new()),
            next_id: AtomicU64::new(0),
        }
    }
}

impl<Req, Res> Chain<Req, Res>
where
    Req: Send + 'static,
    Res: Send + 'static,
{
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a layer inside all existing ones (closest to the terminal).
    pub fn push(&self, layer: impl Layer<Req, Res>) -> Registration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.layers
            .write()
            .expect("chain lock poisoned")
            .push((id, Arc::new(layer)));
        Registration(id)
    }

    /// Adds a layer outside all existing ones. Reserve this for policy that
    /// must see the request before anything else does.
    pub fn prepend(&self, layer: impl Layer<Req, Res>) -> Registration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.layers
            .write()
            .expect("chain lock poisoned")
            .insert(0, (id, Arc::new(layer)));
        Registration(id)
    }

    /// Returns whether the layer was still registered.
    pub fn remove(&self, registration: Registration) -> bool {
        let mut layers = self.layers.write().expect("chain lock poisoned");
        let before = layers.len();
        layers.retain(|(id, _)| *id != registration.0);
        layers.len() != before
    }

    /// Runs `req` through the layers registered when the call started, ending
    /// in `terminal`.
    pub async fn run<F, Fut>(&self, req: Req, terminal: F) -> Res
    where
        F: Fn(Req) -> Fut + Send + Sync,
        Fut: Future<Output = Res> + Send,
    {
        let layers: Vec<Arc<dyn Layer<Req, Res>>> = self
            .layers
            .read()
            .expect("chain lock poisoned")
            .iter()
            .map(|(_, layer)| Arc::clone(layer))
            .collect();
        let terminal = |req: Req| -> BoxFuture<'_, Res> { Box::pin(terminal(req)) };
        Next {
            rest: &layers,
            terminal: &terminal,
        }
        .run(req)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tag(&'static str);

    impl Layer<String, String> for Tag {
        fn call<'a>(
            &'a self,
            req: String,
            next: Next<'a, String, String>,
        ) -> BoxFuture<'a, String> {
            Box::pin(async move {
                let res = next.run(format!("{req}>{}", self.0)).await;
                format!("{res}<{}", self.0)
            })
        }
    }

    struct Deny;

    impl Layer<String, String> for Deny {
        fn call<'a>(
            &'a self,
            _req: String,
            _next: Next<'a, String, String>,
        ) -> BoxFuture<'a, String> {
            Box::pin(async { "denied".to_string() })
        }
    }

    async fn echo(req: String) -> String {
        format!("[{req}]")
    }

    #[tokio::test]
    async fn layers_wrap_the_terminal_outermost_first() {
        let chain = Chain::new();
        chain.push(Tag("a"));
        chain.push(Tag("b"));
        chain.prepend(Tag("outer"));

        assert_eq!(
            chain.run("req".to_string(), echo).await,
            "[req>outer>a>b]<b<a<outer"
        );
    }

    #[tokio::test]
    async fn a_layer_can_short_circuit_and_be_removed() {
        let chain = Chain::new();
        chain.push(Tag("a"));
        let deny = chain.push(Deny);
        chain.push(Tag("never"));

        assert_eq!(chain.run("req".to_string(), echo).await, "denied<a");

        assert!(chain.remove(deny));
        assert_eq!(
            chain.run("req".to_string(), echo).await,
            "[req>a>never]<never<a"
        );
    }

    #[tokio::test]
    async fn an_empty_chain_runs_the_terminal() {
        let chain = Chain::<String, String>::new();
        assert_eq!(chain.run("req".to_string(), echo).await, "[req]");
    }
}
