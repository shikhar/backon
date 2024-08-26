Retry with specify retryable error by `when`.

```rust
use anyhow::Result;
use backon::ExponentialBuilder;
use backon::Retryable;

async fn fetch() -> Result<String> {
    Ok("Hello, Workd!".to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let content = fetch
        .retry(ExponentialBuilder::default())
        .when(|e| e.to_string() == "retryable")
        .await?;
    println!("fetch succeeded: {}", content);
    Ok(())
}
```