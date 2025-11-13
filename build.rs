fn main() -> Result<(), Box<dyn std::error::Error>> {
    shadow()?;

    Ok(())
}

fn shadow() -> shadow_rs::SdResult<()> {
    shadow_rs::ShadowBuilder::builder().build()?;
    Ok(())
}
