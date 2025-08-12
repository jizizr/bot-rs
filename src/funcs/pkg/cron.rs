use crate::{BotError, funcs::ErrorHandler};
use chrono::Local;
use std::{future::Future, str::FromStr, sync::Arc};
pub trait TaskFunc: Send + Sync + 'static {
    type Fut: Future<Output = Result<(), Vec<BotError>>> + Send;

    fn call(&self) -> Self::Fut;
}

impl<F, Fut> TaskFunc for F
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), Vec<BotError>>> + Send,
{
    type Fut = Fut;

    fn call(&self) -> Self::Fut {
        (self)()
    }
}

pub async fn run<E>(
    exp: &'static str,
    f: impl TaskFunc,
    handler: Option<Arc<dyn ErrorHandler<BotError> + Send + Sync>>,
) {
    tokio::task::spawn(async move {
        let schedule = cron::Schedule::from_str(exp).unwrap();
        loop {
            let now = Local::now();
            let next = schedule.upcoming(Local).next().unwrap();
            let wait_time = next.signed_duration_since(now).to_std().unwrap();
            tokio::time::sleep(wait_time).await;
            {
                if let Err(err) = f.call().await {
                    match handler {
                        Some(ref h) => {
                            for e in err {
                                h.clone().handle_error(e).await;
                            }
                        }
                        None => {
                            for e in err {
                                log::error!("Cron task error: {}", e);
                            }
                        }
                    }
                }
            }
        }
    });
}
