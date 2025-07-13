use chrono::{NaiveDate, NaiveDateTime};
use clap::Parser;
use filetime::{FileTime, set_file_times};
use exif::{In, Reader, Tag, Value};
use log::{info, warn};
use regex::Regex;
use std::fs;
use std::fs::File;
use std::process::Command;
use walkdir::WalkDir;
use std::path::Path;
use pretty_env_logger;
use rayon::prelude::*;

/// Command line arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input directory path
    #[arg(long)]
    input: String,

    /// Output directory path
    #[arg(long)]
    output: Option<String>,

    /// Dry run mode: no changes will be made
    #[arg(long)]
    dry_run: bool,
}

fn main() {
    pretty_env_logger::init();
    let args = Args::parse();
    println!("Input directory: {}", args.input);
    if args.dry_run {
        println!("Dry run mode: no changes will be made.");
    }

    // Regex patterns for IMG and VID files
    let img_pattern = Regex::new(r"^IMG_(\d{8})_(\d{6})\d*.*\.jpg$").unwrap();
    let vid_pattern = Regex::new(r"^VID_(\d{8})_(\d{6})\d*.*\.mp4$").unwrap();
    let img_date_only_pattern = Regex::new(r"^IMG-(\d{8})-WA\d+.*\.jpg$").unwrap();
    let screenshot_pattern = Regex::new(r"^Screenshot_(\d{8})-(\d{6}).*\.jpg$").unwrap();

    let mut matched_files = Vec::new();
    for entry in WalkDir::new(&args.input).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let fname = entry.file_name().to_string_lossy();
            // Only match files with a date in the name
            let is_img_with_date = img_pattern.is_match(&fname)
                || img_date_only_pattern.is_match(&fname)
                || screenshot_pattern.is_match(&fname);
            let is_vid_with_date = vid_pattern.is_match(&fname);
            if is_img_with_date || is_vid_with_date {
                matched_files.push(entry.path().display().to_string());
            }
        }
    }
    println!("Matched files:");
    // Use rayon for parallel file processing
    matched_files.par_iter().for_each(|file| {
        let fname = std::path::Path::new(file)
            .file_name()
            .map(|f| f.to_string_lossy())
            .unwrap_or_default();
        let mut date = "unknown".to_string();
        let mut time = "unknown".to_string();
        if let Some(caps) = img_pattern.captures(&fname) {
            date = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or(date.clone());
            time = caps.get(2).map(|m| m.as_str().to_string()).unwrap_or(time.clone());
        } else if let Some(caps) = vid_pattern.captures(&fname) {
            date = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or(date.clone());
            time = caps.get(2).map(|m| m.as_str().to_string()).unwrap_or(time.clone());
        } else if let Some(caps) = img_date_only_pattern.captures(&fname) {
            date = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or(date.clone());
        } else if let Some(caps) = screenshot_pattern.captures(&fname) {
            date = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or(date.clone());
            time = caps.get(2).map(|m| m.as_str().to_string()).unwrap_or(time.clone());
        }
        println!("File: {} | Date: {} | Time: {}", fname, date, time);

        // Only process JPG files for EXIF
        if fname.to_lowercase().ends_with(".jpg") && date != "unknown" {
            let file_handle = File::open(&file);
            if let Ok(fh) = file_handle {
                let mut buf_reader = std::io::BufReader::new(fh);
                let exifreader = Reader::new();
                let exif = exifreader.read_from_container(&mut buf_reader);
                if let Ok(exif) = exif {
                    let exif_date = exif
                        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
                        .and_then(|field| match &field.value {
                            Value::Ascii(vec) if !vec.is_empty() => {
                                let s = String::from_utf8_lossy(&vec[0]);
                                chrono::NaiveDateTime::parse_from_str(&s, "%Y:%m:%d %H:%M:%S").ok()
                            }
                            _ => None,
                        });
                    let parsed_date = if time != "unknown" {
                        chrono::NaiveDateTime::parse_from_str(
                            &format!("{} {}", date, time),
                            "%Y%m%d %H%M%S",
                        )
                        .ok()
                    } else {
                        chrono::NaiveDate::parse_from_str(&date, "%Y%m%d")
                            .ok()
                            .and_then(|d| d.and_hms_opt(0, 0, 0))
                    };
                    match (parsed_date, exif_date) {
                        (Some(parsed), Some(exif_dt)) => {
                            if parsed < exif_dt {
                                if args.dry_run {
                                    info!(
                                        "[DRY RUN] Would modify EXIF date for file: {} from {} to {}",
                                        file, exif_dt, parsed
                                    );
                                } else {
                                    // Set EXIF date to parsed
                                    match set_exif_date(&file, parsed) {
                                        Ok(_) => info!(
                                            "Modified EXIF date for file: {} from {} to {}",
                                            file, exif_dt, parsed
                                        ),
                                        Err(e) => warn!(
                                            "Failed to modify EXIF date for file: {}: {}",
                                            file, e
                                        ),
                                    }
                                }
                            } else {
                                info!("No change needed for file: {}", file);
                            }
                        }
                        _ => {
                            warn!("Could not parse date for file: {}", file);
                        }
                    }
                } else {
                    warn!("No EXIF data found for file: {}", file);
                    // Try to set file creation time if we have a valid date
                    let parsed_date = if time != "unknown" {
                        NaiveDateTime::parse_from_str(
                            &format!("{} {}", date, time),
                            "%Y%m%d %H%M%S",
                        )
                        .ok()
                    } else {
                        NaiveDate::parse_from_str(&date, "%Y%m%d")
                            .ok()
                            .and_then(|d| d.and_hms_opt(0, 0, 0))
                    };
                    if let Some(parsed) = parsed_date {
                        if args.dry_run {
                            info!(
                                "[DRY RUN] Would set file creation time for file: {} to {}",
                                file, parsed
                            );
                        } else {
                            match set_file_creation_time(&file, parsed) {
                                Ok(_) => {
                                    info!("Set file creation time for file: {} to {}", file, parsed)
                                }
                                Err(e) => warn!(
                                    "Failed to set file creation time for file: {}: {}",
                                    file, e
                                ),
                            }
                        }
                    } else {
                        warn!("Could not parse date for file: {}", file);
                    }
                }
            } else {
                warn!("Could not open file: {}", file);
            }
        }
        // Move all processed files to output directory if specified and not in dry-run mode
        if let Some(ref out_dir) = args.output {
            let out_path = Path::new(out_dir).join(fname.as_ref());
            if args.dry_run {
                info!("[DRY RUN] Would move file: {} to {}", file, out_path.display());
            } else {
                match fs::rename(&file, &out_path) {
                    Ok(_) => info!("Moved file: {} to {}", file, out_path.display()),
                    Err(e) => warn!("Failed to move file: {} to {}: {}", file, out_path.display(), e),
                }
            }
        }
    });
}

fn set_exif_date(file_path: &str, new_date: NaiveDateTime) -> Result<(), String> {
    let formatted = new_date.format("%Y:%m:%d %H:%M:%S").to_string();
    let status = Command::new("exiftool")
        .arg("-DateTimeOriginal=".to_owned() + &formatted)
        .arg(file_path)
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("exiftool failed with status: {}", s)),
        Err(e) => Err(format!("Failed to run exiftool: {}", e)),
    }
}

fn set_file_creation_time(file_path: &str, new_date: NaiveDateTime) -> Result<(), String> {
    let ft = FileTime::from_unix_time(new_date.and_utc().timestamp(), 0);
    let meta = fs::metadata(file_path).map_err(|e| e.to_string())?;
    let atime = FileTime::from_last_access_time(&meta);
    set_file_times(file_path, atime, ft).map_err(|e| e.to_string())?;
    Ok(())
}
