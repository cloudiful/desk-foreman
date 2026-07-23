use std::{future::Future, pin::Pin};

pub type RunnerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
