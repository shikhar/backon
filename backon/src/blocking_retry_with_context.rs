use core::time::Duration;

use crate::backoff::BackoffBuilder;
use crate::blocking_sleep::MaybeBlockingSleeper;
use crate::{Backoff, BlockingSleeper, DefaultBlockingSleeper};

/// BlockingRetryableWithContext adds retry support for blocking functions.
pub trait BlockingRetryableWithContext<
    B: BackoffBuilder,
    T,
    E,
    Ctx,
    F: FnMut(Ctx) -> (Ctx, Result<T, E>),
>
{
    /// Generate a new retry
    fn retry(self, builder: B) -> BlockingRetryWithContext<B::Backoff, T, E, Ctx, F>;
}

impl<B, T, E, Ctx, F> BlockingRetryableWithContext<B, T, E, Ctx, F> for F
where
    B: BackoffBuilder,
    F: FnMut(Ctx) -> (Ctx, Result<T, E>),
{
    fn retry(self, builder: B) -> BlockingRetryWithContext<B::Backoff, T, E, Ctx, F> {
        BlockingRetryWithContext::new(self, builder.build())
    }
}

/// Retry structure generated by [`BlockingRetryableWithContext`].
pub struct BlockingRetryWithContext<
    B: Backoff,
    T,
    E,
    Ctx,
    F: FnMut(Ctx) -> (Ctx, Result<T, E>),
    SF: MaybeBlockingSleeper = DefaultBlockingSleeper,
    RF = fn(&E) -> bool,
    NF = fn(&E, Duration),
> {
    backoff: B,
    retryable: RF,
    notify: NF,
    f: F,
    sleep_fn: SF,
    ctx: Option<Ctx>,
}

impl<B, T, E, Ctx, F> BlockingRetryWithContext<B, T, E, Ctx, F>
where
    B: Backoff,
    F: FnMut(Ctx) -> (Ctx, Result<T, E>),
{
    /// Create a new retry.
    fn new(f: F, backoff: B) -> Self {
        BlockingRetryWithContext {
            backoff,
            retryable: |_: &E| true,
            notify: |_: &E, _: Duration| {},
            sleep_fn: DefaultBlockingSleeper::default(),
            f,
            ctx: None,
        }
    }
}

impl<B, T, E, Ctx, F, SF, RF, NF> BlockingRetryWithContext<B, T, E, Ctx, F, SF, RF, NF>
where
    B: Backoff,
    F: FnMut(Ctx) -> (Ctx, Result<T, E>),
    SF: MaybeBlockingSleeper,
    RF: FnMut(&E) -> bool,
    NF: FnMut(&E, Duration),
{
    /// Set the context for retrying.
    ///
    /// Context is used to capture ownership manually to prevent lifetime issues.
    pub fn context(self, context: Ctx) -> BlockingRetryWithContext<B, T, E, Ctx, F, SF, RF, NF> {
        BlockingRetryWithContext {
            backoff: self.backoff,
            retryable: self.retryable,
            notify: self.notify,
            f: self.f,
            sleep_fn: self.sleep_fn,
            ctx: Some(context),
        }
    }

    /// Set the sleeper for retrying.
    ///
    /// The sleeper should implement the [`BlockingSleeper`] trait. The simplest way is to use a closure like  `Fn(Duration)`.
    ///
    /// If not specified, we use the [`DefaultBlockingSleeper`].
    pub fn sleep<SN: BlockingSleeper>(
        self,
        sleep_fn: SN,
    ) -> BlockingRetryWithContext<B, T, E, Ctx, F, SN, RF, NF> {
        BlockingRetryWithContext {
            backoff: self.backoff,
            retryable: self.retryable,
            notify: self.notify,
            f: self.f,
            sleep_fn,
            ctx: self.ctx,
        }
    }

    /// Set the conditions for retrying.
    ///
    /// If not specified, all errors are considered retryable.
    pub fn when<RN: FnMut(&E) -> bool>(
        self,
        retryable: RN,
    ) -> BlockingRetryWithContext<B, T, E, Ctx, F, SF, RN, NF> {
        BlockingRetryWithContext {
            backoff: self.backoff,
            retryable,
            notify: self.notify,
            f: self.f,
            sleep_fn: self.sleep_fn,
            ctx: self.ctx,
        }
    }

    /// Set to notify for all retry attempts.
    ///
    /// When a retry happens, the input function will be invoked with the error and the sleep duration before pausing.
    ///
    /// If not specified, this operation does nothing.
    pub fn notify<NN: FnMut(&E, Duration)>(
        self,
        notify: NN,
    ) -> BlockingRetryWithContext<B, T, E, Ctx, F, SF, RF, NN> {
        BlockingRetryWithContext {
            backoff: self.backoff,
            retryable: self.retryable,
            notify,
            f: self.f,
            sleep_fn: self.sleep_fn,
            ctx: self.ctx,
        }
    }
}

impl<B, T, E, Ctx, F, SF, RF, NF> BlockingRetryWithContext<B, T, E, Ctx, F, SF, RF, NF>
where
    B: Backoff,
    F: FnMut(Ctx) -> (Ctx, Result<T, E>),
    SF: BlockingSleeper,
    RF: FnMut(&E) -> bool,
    NF: FnMut(&E, Duration),
{
    /// Call the retried function.
    ///
    /// TODO: implement [`FnOnce`] after it stable.
    pub fn call(mut self) -> (Ctx, Result<T, E>) {
        let mut ctx = self.ctx.take().expect("context must be valid");
        loop {
            let (xctx, result) = (self.f)(ctx);
            // return ctx ownership back
            ctx = xctx;

            match result {
                Ok(v) => return (ctx, Ok(v)),
                Err(err) => {
                    if !(self.retryable)(&err) {
                        return (ctx, Err(err));
                    }

                    match self.backoff.next() {
                        None => return (ctx, Err(err)),
                        Some(dur) => {
                            (self.notify)(&err, dur);
                            self.sleep_fn.sleep(dur);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExponentialBuilder;
    use alloc::string::ToString;
    use anyhow::anyhow;
    use anyhow::Result;
    use core::time::Duration;
    use spin::Mutex;

    struct Test;

    impl Test {
        fn hello(&mut self) -> Result<usize> {
            Err(anyhow!("not retryable"))
        }
    }

    #[test]
    fn test_retry_with_not_retryable_error() -> Result<()> {
        let error_times = Mutex::new(0);

        let test = Test;

        let backoff = ExponentialBuilder::default().with_min_delay(Duration::from_millis(1));

        let (_, result) = {
            |mut v: Test| {
                let mut x = error_times.lock();
                *x += 1;

                let res = v.hello();
                (v, res)
            }
        }
        .retry(backoff)
        .context(test)
        // Only retry If error message is `retryable`
        .when(|e| e.to_string() == "retryable")
        .call();

        assert!(result.is_err());
        assert_eq!("not retryable", result.unwrap_err().to_string());
        // `f` always returns error "not retryable", so it should be executed
        // only once.
        assert_eq!(*error_times.lock(), 1);
        Ok(())
    }
}
