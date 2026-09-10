use anyhow::{bail, Result};
use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand};

use epaper::geometry::{DEFAULT_COLUMNS, DEFAULT_ROWS};
use epaper::image::{self, Fit, PackOptions};
use epaper::protocol as p;
use epaper::transport::Transport;

#[derive(Parser)]
#[command(
    name = "epaper",
    about = "Host CLI for the Pico 2 driven Waveshare e-paper panel"
)]
struct Cli {
    #[arg(long, global = true)]
    port: Option<String>,
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    verbose: bool,
    #[command(subcommand)]
    action: Action,
}

#[derive(Args, Clone)]
struct ConsoleOptions {
    /// terminal grid, e.g. 80x24
    #[arg(long, value_name = "COLSxROWS", default_value_t = format!("{DEFAULT_COLUMNS}x{DEFAULT_ROWS}"))]
    size: String,
    /// console font file (default: $EPAPER_FONT or a discovered Nerd Font)
    #[arg(long, value_name = "PATH")]
    font: Option<String>,
}

#[derive(Subcommand)]
enum Action {
    Ping,
    Info,
    Sleep,
    Bootsel,
    Status,
    Clear {
        #[arg(long, action = ArgAction::SetTrue)]
        black: bool,
    },
    Refresh {
        #[arg(long, action = ArgAction::SetTrue)]
        full: bool,
    },
    Text {
        text: String,
        #[command(flatten)]
        console: ConsoleOptions,
    },
    #[command(group(ArgGroup::new("source").required(true).args(["image", "testcard"])))]
    #[command(group(ArgGroup::new("mono").args(["dither", "threshold"])))]
    Draw {
        image: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        testcard: bool,
        #[arg(long, value_parser = ["fill", "fit", "stretch"], default_value = "fit")]
        fit: String,
        #[arg(long, value_parser = ["0", "90", "180", "270"], default_value = "0")]
        rotate: String,
        #[arg(long, action = ArgAction::SetTrue)]
        dither: bool,
        #[arg(long, default_value_t = 128)]
        threshold: u8,
        #[arg(long, action = ArgAction::SetTrue)]
        invert: bool,
        #[arg(long, action = ArgAction::SetTrue)]
        partial: bool,
    },
    Console {
        #[arg(long, action = ArgAction::SetTrue)]
        stdin: bool,
        #[arg(long, action = ArgAction::SetTrue)]
        echo: bool,
        #[command(flatten)]
        console: ConsoleOptions,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(error) => {
            eprintln!("epaper: {error}");
            std::process::ExitCode::from(1)
        }
    }
}

fn run(cli: &Cli) -> Result<u8> {
    let mut device = Transport::open(cli.port.as_deref(), cli.verbose)?;
    match &cli.action {
        Action::Bootsel => {
            device.bootsel()?;
        }
        Action::Ping | Action::Info | Action::Sleep => {
            let (command, payload) = match cli.action {
                Action::Ping => p::ping(),
                Action::Info => p::info(),
                _ => p::sleep(),
            };
            let response = device.request(command, &payload)?;
            if !response.is_empty() {
                println!("{}", String::from_utf8_lossy(&response));
            }
        }
        Action::Status => {
            let (command, payload) = p::status();
            let response = device.request(command, &payload)?;
            println!("{}", p::parse_status(&response)?);
        }
        Action::Refresh { full } => {
            let (command, payload) = p::refresh(*full);
            device.request(command, &payload)?;
        }
        Action::Clear { black } => {
            let (command, payload) = p::clear(*black);
            device.request(command, &payload)?;
        }
        Action::Draw {
            image: path,
            testcard,
            fit,
            rotate,
            dither,
            threshold,
            invert,
            partial,
        } => {
            let options = PackOptions {
                fit: match fit.as_str() {
                    "fill" => Fit::Fill,
                    "stretch" => Fit::Stretch,
                    _ => Fit::Fit,
                },
                rotate: rotate.parse().expect("clap restricts the rotation"),
                dither: *dither,
                threshold: *threshold,
                invert: *invert,
            };
            let data = if *testcard {
                let card = image::testcard()?;
                image::pack(
                    &::image::DynamicImage::ImageLuma8(card).into_rgba8(),
                    &options,
                )?
            } else {
                let path = path.as_deref().expect("clap requires a source");
                image::load(path, &options)?
            };
            let (command, payload) = p::img_begin();
            device.request(command, &payload)?;
            for offset in (0..data.len()).step_by(p::IMG_DATA_MAX) {
                let chunk = &data[offset..(offset + p::IMG_DATA_MAX).min(data.len())];
                let (command, payload) = p::img_data(offset as u32, chunk)?;
                device.request(command, &payload)?;
                eprint!("\rUploaded {}/{} bytes", offset + chunk.len(), data.len());
            }
            eprintln!("\nRefreshing...");
            let (command, payload) = p::img_end(*partial);
            device.request(command, &payload)?;
        }
        Action::Text { .. } => {
            bail!("`text` lands with the terminal model in the next change")
        }
        Action::Console { stdin, command, .. } => {
            if *stdin && !command.is_empty() {
                bail!("--stdin cannot be combined with a command");
            }
            bail!("`console` lands with the terminal model in the next change")
        }
    }
    Ok(0)
}
