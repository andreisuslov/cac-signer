//! Page previews drawn by PDFKit, the engine behind Preview, so filled-in form values and
//! signature stamps look the way they do there.

use objc2::rc::autoreleasepool;
use objc2::AllocAnyThread;
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize};
use objc2_pdf_kit::{PDFDisplayBox, PDFDocument};

/// PNG of page `number` (from 1), `width` pixels wide, showing the crop box the way it is
/// displayed (rotation applied).
pub fn page_png(pdf: &[u8], number: usize, width: usize) -> Result<Vec<u8>, String> {
    let failed = || "Could not draw the page.".to_string();
    let (pixels, height) = autoreleasepool(|_| {
        let data = NSData::with_bytes(pdf);
        let doc = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }.ok_or_else(failed)?;
        let page = number
            .checked_sub(1)
            .and_then(|index| unsafe { doc.pageAtIndex(index) })
            .ok_or_else(|| format!("The PDF has no page {number}."))?;
        let bounds = unsafe { page.boundsForBox(PDFDisplayBox::CropBox) };
        let turned = unsafe { page.rotation() }.rem_euclid(180) == 90;
        let (w, h) = if turned {
            (bounds.size.height, bounds.size.width)
        } else {
            (bounds.size.width, bounds.size.height)
        };
        if w <= 0.0 || h <= 0.0 {
            return Err(failed());
        }
        let scale = width as f64 / w;
        let height = ((h * scale).round() as usize).clamp(1, 20_000);

        let mut pixels = vec![0u8; width * height * 4];
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                space.as_deref(),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }
        .ok_or_else(failed)?;
        CGContext::set_rgb_fill_color(Some(&context), 1.0, 1.0, 1.0, 1.0);
        CGContext::fill_rect(
            Some(&context),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width as f64, height as f64)),
        );
        CGContext::scale_ctm(Some(&context), scale, scale);
        // Draws the page rotated as displayed, with the box's corner at the origin.
        unsafe { page.drawWithBox_toContext(PDFDisplayBox::CropBox, &context) };
        drop(context);
        Ok((pixels, height))
    })?;

    // The page was painted on white, so dropping alpha loses nothing.
    let rgb: Vec<u8> = pixels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&rgb))
        .map_err(|e| format!("Could not draw the page: {e}"))?;
    Ok(png)
}
