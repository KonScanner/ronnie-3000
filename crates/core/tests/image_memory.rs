//! The memory of one image decode, measured with a counting allocator in an isolated binary.

#![allow(unsafe_code)]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    io::Cursor,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use image::{
    ColorType, ExtendedColorType, ImageEncoder, ImageFormat, RgbImage, RgbaImage,
    codecs::{jpeg::JpegEncoder, webp::WebPEncoder},
};
use ronie_core::{
    DecodedImage, DownloadClient, DownloadError, HttpResponse, OriginPolicy, RecordedTransport,
};
use url::Url;

struct PeakAllocator;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn add(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::SeqCst) + size;
    PEAK.fetch_max(live, Ordering::SeqCst);
}

// SAFETY: each method delegates to the system allocator with the original layout.
unsafe impl GlobalAlloc for PeakAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add(layout.size());
        // SAFETY: the caller gives a valid layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        add(layout.size());
        // SAFETY: the caller gives a valid layout.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the caller gives a pointer of this allocator with its layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: PeakAllocator = PeakAllocator;

const MEBIBYTE: usize = 1024 * 1024;

/// A measurement needs the allocator for itself, so the tests of this binary run one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Decodes one answer, and returns the result with the peak of new memory that the decode took.
fn measured_decode(body: Vec<u8>) -> (Result<Option<DecodedImage>, DownloadError>, usize) {
    let origin = Url::parse("https://images.example").expect("origin");
    let client = DownloadClient::new(
        RecordedTransport::new(vec![Ok(HttpResponse::new(200, body))]),
        OriginPolicy::new(origin.clone(), []),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let url = origin.join("cover").expect("URL");
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let result = runtime.block_on(client.download_image(&url));
    let peak = PEAK.load(Ordering::SeqCst) - before;
    (result, peak)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn png_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&u32::try_from(data.len()).expect("length").to_be_bytes());
    let start = output.len();
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let crc = crc32(&output[start..]);
    output.extend_from_slice(&crc.to_be_bytes());
}

/// A small file whose header claims these dimensions and three bytes for each pixel.
fn lying_png(width: u32, height: u32) -> Vec<u8> {
    let mut output = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut output, b"IHDR", &header);
    png_chunk(&mut output, b"IDAT", &[0x78, 0x01, 0x01, 0, 0, 0xff, 0xff]);
    png_chunk(&mut output, b"IEND", &[]);
    output
}

fn small_jpeg() -> Vec<u8> {
    let mut output = Vec::new();
    JpegEncoder::new(&mut output)
        .write_image(&[128; 16 * 16 * 3], 16, 16, ExtendedColorType::Rgb8)
        .expect("JPEG fixture");
    output
}

/// A small file whose frame header claims these dimensions.
fn lying_jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut output = small_jpeg();
    let frame = output
        .windows(2)
        .position(|marker| marker == [0xFF, 0xC0])
        .expect("frame header");
    output[frame + 5..frame + 7].copy_from_slice(&height.to_be_bytes());
    output[frame + 7..frame + 9].copy_from_slice(&width.to_be_bytes());
    output
}

/// A small file whose lossless header claims these dimensions.
fn lying_webp(width: u32, height: u32) -> Vec<u8> {
    let mut output = Vec::new();
    WebPEncoder::new_lossless(&mut output)
        .write_image(&[128; 16 * 16 * 3], 16, 16, ExtendedColorType::Rgb8)
        .expect("WebP fixture");
    assert_eq!(&output[12..16], b"VP8L");
    assert_eq!(output[20], 0x2F);
    let bits = u32::from_le_bytes(output[21..25].try_into().expect("four bytes"));
    let bits = (bits & 0xF000_0000) | (width - 1) | ((height - 1) << 14);
    output[21..25].copy_from_slice(&bits.to_le_bytes());
    output
}

#[test]
fn a_header_over_the_pixel_limit_is_refused_before_any_pixel_allocation() {
    let _alone = one_at_a_time();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("PNG at the old limit", lying_png(8192, 8192)),
        ("PNG one column too wide", lying_png(4097, 4096)),
        ("PNG in a long strip", lying_png(8192, 2049)),
        ("JPEG at the old limit", lying_jpeg(8192, 8192)),
        ("JPEG one column too wide", lying_jpeg(4097, 4096)),
        ("WebP at the old limit", lying_webp(8192, 8192)),
        ("WebP one column too wide", lying_webp(4097, 4096)),
    ];

    for (name, body) in cases {
        let (result, peak) = measured_decode(body);

        assert!(matches!(result, Err(DownloadError::Image)), "{name}");
        assert!(
            peak < MEBIBYTE,
            "{name} took {} KiB before the refusal",
            peak / 1024
        );
    }
}

#[test]
fn a_cover_at_the_pixel_limit_decodes_inside_its_memory_bound() {
    let _alone = one_at_a_time();
    let encode_png = |image: Result<RgbaImage, RgbImage>| {
        let mut output = Cursor::new(Vec::new());
        match image {
            Ok(image) => image.write_to(&mut output, ImageFormat::Png),
            Err(image) => image.write_to(&mut output, ImageFormat::Png),
        }
        .expect("PNG fixture");
        output.into_inner()
    };
    let cases: [(&str, Vec<u8>, usize); 2] = [
        (
            "RGBA",
            encode_png(Ok(RgbaImage::new(4096, 4096))),
            // Four bytes for each pixel, with no conversion.
            72 * MEBIBYTE,
        ),
        (
            "RGB",
            encode_png(Err(RgbImage::new(4096, 4096))),
            // Three bytes for each pixel, while the conversion writes four.
            120 * MEBIBYTE,
        ),
    ];

    for (name, body, bound) in cases {
        let (result, peak) = measured_decode(body);

        let image = result.expect(name).expect(name);
        assert_eq!((image.width, image.height), (4096, 4096), "{name}");
        assert_eq!(image.rgba.len(), 4096 * 4096 * 4, "{name}");
        assert!(peak < bound, "{name} took {} MiB", peak / MEBIBYTE);
    }
}

#[test]
fn a_real_cover_in_each_format_still_decodes() {
    let _alone = one_at_a_time();
    let pixels = vec![96_u8; 3000 * 3000 * 3];
    let mut jpeg = Vec::new();
    JpegEncoder::new(&mut jpeg)
        .encode(&pixels, 3000, 3000, ColorType::Rgb8.into())
        .expect("JPEG fixture");
    let mut webp = Vec::new();
    WebPEncoder::new_lossless(&mut webp)
        .write_image(&pixels, 3000, 3000, ExtendedColorType::Rgb8)
        .expect("WebP fixture");

    for (name, body) in [("JPEG", jpeg), ("WebP", webp)] {
        let (result, _) = measured_decode(body);

        let image = result.expect(name).expect(name);
        assert_eq!((image.width, image.height), (3000, 3000), "{name}");
    }
}
