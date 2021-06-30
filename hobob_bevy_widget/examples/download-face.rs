use std::result::Result;
use futures_util::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = reqwest::get("http://i0.hdslb.com/bfs/face/42eb05e354476c2b22b5c512c4a484d93650020c.jpg")
        .await?
        .bytes_stream();
    let p = std::path::Path::new(".cache/quin/face.jpg");
    std::fs::DirBuilder::new().recursive(true).create(p.parent().unwrap())?;
    let mut f = std::fs::File::create(p)?;
    while let Some(item) = stream.next().await {
        f.write_all(item?.as_ref())?;
    }
    Ok(())
}
