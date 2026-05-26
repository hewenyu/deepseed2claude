#[tokio::main]
async fn main() -> Result<(), deepseed2claude::Error> {
    deepseed2claude::run().await
}
