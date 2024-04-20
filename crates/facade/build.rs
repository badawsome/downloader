fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().compile(&["src/facade/items.proto", "src/base.proto"], &["src"])?;
    Ok(())
}
