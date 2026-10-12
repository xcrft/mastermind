//! Experimental stdin wrapper around the native owning implementation.
#[path = "../../../../mcp/servers/mmcg/src/repo_map.rs"]
mod repo_map;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let result = repo_map::from_reader(std::io::stdin())?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
