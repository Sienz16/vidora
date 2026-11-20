use std::{path::PathBuf, process::ExitCode};

use tokio::fs::File;
use vidora::{Processor, Tools};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next(), args.next()) {
        (Some(input), Some(destination), None) => {
            match convert(input.into(), destination.into()).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(reason) => {
                    eprintln!("{reason}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("Usage: vidora INPUT_MEDIA EXISTING_OUTPUT_DIRECTORY");
            ExitCode::from(2)
        }
    }
}

async fn convert(input: PathBuf, destination: PathBuf) -> Result<(), String> {
    let metadata = tokio::fs::metadata(&input)
        .await
        .map_err(|_| "input_unavailable")?;
    if !metadata.is_file() {
        return Err("input_must_be_a_regular_file".into());
    }
    let file = File::open(input).await.map_err(|_| "input_unavailable")?;
    let tools = Tools::from_path().map_err(|_| "media_tools_unavailable")?;
    let mut processor =
        Processor::new(tools, &std::env::temp_dir()).map_err(|_| "work_directory_unavailable")?;
    let job = processor
        .process(file, Some(metadata.len()))
        .await
        .map_err(|record| serde_json::to_string(&record).expect("serializable job record"))?;
    let record = serde_json::to_string(job.record()).expect("serializable job record");
    let output = job
        .export(&destination)
        .await
        .map_err(|record| serde_json::to_string(&record).expect("serializable job record"))?;
    println!("{record}");
    println!("{}", output.display());
    Ok(())
}
