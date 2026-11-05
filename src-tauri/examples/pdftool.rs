//! Runs the app's PDF code from the command line, for tests/formats/run.py. No card needed:
//! `sign` gets its CMS signature from a command that reads the signed bytes on stdin.
//!
//!   pdftool inspect <in.pdf>
//!   pdftool pdfkit-sizes <in.pdf>
//!   pdftool render <in.pdf> <page> <width> <out.png>
//!   pdftool add <in.pdf> <out.pdf> <new-fields.json>
//!   pdftool sign <in.pdf> <out.pdf> <new-fields.json> <field> <signer> <cms command> [args...]

use std::io::Write;
use std::process::{exit, Command, Stdio};

use cac_signer_lib::{pdf, render};
use objc2::AllocAnyThread;
use objc2_foundation::NSData;
use objc2_pdf_kit::{PDFDisplayBox, PDFDocument};

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{path}: {e}"))
}

fn new_fields(path: &str) -> Result<Vec<pdf::NewField>, String> {
    serde_json::from_slice(&read(path)?).map_err(|e| format!("{path}: {e}"))
}

/// Page sizes as PDFKit displays them, to compare with the app's own page geometry.
fn pdfkit_sizes(pdf: &[u8]) -> Result<Vec<[f64; 2]>, String> {
    let doc = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &NSData::with_bytes(pdf)) }
        .ok_or("PDFKit could not open the file")?;
    Ok((0..unsafe { doc.pageCount() })
        .filter_map(|i| unsafe { doc.pageAtIndex(i) })
        .map(|page| {
            let size = unsafe { page.boundsForBox(PDFDisplayBox::CropBox) }.size;
            if unsafe { page.rotation() }.rem_euclid(180) == 90 {
                [size.height, size.width]
            } else {
                [size.width, size.height]
            }
        })
        .collect())
}

fn cms_from(command: &[&str], data: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = Command::new(command[0])
        .args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", command[0]))?;
    child.stdin.take().unwrap().write_all(data).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("{} failed", command[0]));
    }
    Ok(out.stdout)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["inspect", input] => read(input)
            .and_then(|pdf| pdf::inspect(&pdf))
            .map(|info| println!("{}", serde_json::to_string(&info).unwrap())),
        ["pdfkit-sizes", input] => read(input)
            .and_then(|pdf| pdfkit_sizes(&pdf))
            .map(|sizes| println!("{}", serde_json::to_string(&sizes).unwrap())),
        ["render", input, page, width, output] => read(input)
            .and_then(|pdf| render::page_png(&pdf, page.parse().unwrap(), width.parse().unwrap()))
            .and_then(|png| write(output, &png)),
        ["add", input, output, fields] => (|| {
            let updated = pdf::add_fields(&read(input)?, &new_fields(fields)?)?;
            write(output, &updated)
        })(),
        ["sign", input, output, fields, field, signer, command @ ..] if !command.is_empty() => (|| {
            let (signed, time) = pdf::sign(
                &read(input)?,
                &new_fields(fields)?,
                field,
                signer,
                || Ok(()),
                |data| cms_from(command, data),
            )?;
            write(output, &signed)?;
            println!("{time}");
            Ok(())
        })(),
        _ => Err("see the usage at the top of examples/pdftool.rs".into()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        exit(1);
    }
}
