use anyhow::Context;
use collab_stream::client::CollabRedisStream;
use collab_stream::metrics::CollabStreamMetrics;
use rand::{thread_rng, Rng};
use std::sync::Arc;

pub async fn redis_client() -> redis::Client {
  let redis_uri = "redis://localhost:6379";
  redis::Client::open(redis_uri)
    .context("failed to connect to redis")
    .unwrap()
}

pub async fn stream_client() -> CollabRedisStream {
  let redis_client = redis_client().await;
  CollabRedisStream::new(redis_client, Arc::new(CollabStreamMetrics::default()))
    .await
    .context("failed to create stream client")
    .unwrap()
}

pub fn random_i64() -> i64 {
  let mut rng = thread_rng();
  let num: i64 = rng.gen();
  num
}
