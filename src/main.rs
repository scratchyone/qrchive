use image::io::Reader as ImageReader;
use itertools::Itertools;
use qrcode::render::svg;
use qrcode::{EcLevel, QrCode, Version};
mod qr_data;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tera::{Context, Tera};
use zbar_rust::ZBarImageScanner;

use clap::Parser;
use colour::*;

#[derive(clap::Parser)]
struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    Encode {
        #[clap(index = 1)]
        input: String,
        #[clap(short, long)]
        output: String,
        #[clap(short, long)]
        rows: usize,
        #[clap(short, long)]
        cols: usize,
        #[clap(short, long)]
        error_correction: String,
        #[clap(short, long)]
        version: u32,
    },
    Decode {
        #[clap(index = 1)]
        input: PathBuf,
        #[clap(short, long)]
        output: String,
    },
}

fn main() {
    let args = Args::parse();
    match args.action {
        Action::Encode {
            input,
            output,
            rows,
            cols,
            error_correction,
            version,
        } => {
            let ec = match error_correction.as_str() {
                "L" => EcLevel::L,
                "M" => EcLevel::M,
                "Q" => EcLevel::Q,
                "H" => EcLevel::H,
                _ => {
                    e_red_ln!("Invalid error correction level, expected L, M, Q or H");
                    std::process::exit(1);
                }
            };
            let version = match version {
                1..=40 => Version::Normal(version as i16),
                _ => {
                    e_red_ln!("Invalid version, expected 1-40");
                    std::process::exit(1);
                }
            };
            encode(&input, &output, rows, cols, ec, version);
        }
        Action::Decode { input, output } => {
            decode(&input, &output);
        }
    }
}
fn get_images_from_dir_or_file(dir: &Path) -> Vec<image::GrayImage> {
    if std::fs::metadata(dir).unwrap().is_dir() {
        blue_ln!("Reading all files in {}", dir.display());
        let files = std::fs::read_dir(dir).unwrap();
        files
            .map(|f| f.unwrap().path())
            .filter(|f| {
                let ext = match f.extension() {
                    Some(v) => v.to_str().unwrap().to_lowercase(),
                    None => "".to_string(),
                };
                ext == "png" || ext == "jpg" || ext == "jpeg"
            })
            .map(|f| {
                blue_ln!("Reading {}", f.display());
                ImageReader::open(f).unwrap().decode().unwrap().to_luma8()
            })
            .collect::<Vec<_>>()
    } else {
        vec![ImageReader::open(dir).unwrap().decode().unwrap().to_luma8()]
    }
}
fn extract_codes_from_image(
    image: &image::GrayImage,
    scanner: &mut ZBarImageScanner,
) -> Vec<zbar_rust::ZBarImageScanResult> {
    let (width, height) = image.dimensions();
    scanner
        .scan_y800(image.clone().into_raw(), width, height)
        .unwrap()
}
fn decode(input: &Path, output: &str) {
    blue_ln!("Decoding {} into {}", input.display(), output);
    let images = get_images_from_dir_or_file(input);
    let mut scanner = ZBarImageScanner::new();
    scanner
        .set_config(
            zbar_rust::ZBarSymbolType::ZBarQRCode,
            zbar_rust::ZBarConfig::ZBarCfgNum,
            1,
        )
        .unwrap();
    let res = images.iter().enumerate().flat_map(|(i, image)| {
        blue_ln!("Processing image {} of {}", i + 1, images.len());
        let results = extract_codes_from_image(image, &mut scanner);
        blue_ln!("Found {} results", results.len());
        results
    });
    fn get_num(s: &[u8]) -> usize {
        let b1 = s[0] as usize;
        let b2 = s[1] as usize;
        (b1 << 8) | b2
    }
    let res: Vec<zbar_rust::ZBarImageScanResult> = res
        .sorted_by(|a, b| {
            let a = get_num(&a.data);
            let b = get_num(&b.data);
            a.cmp(&b)
        })
        .collect();
    let mut res = res.iter().map(|r| &r.data[2..]).collect::<Vec<_>>();
    let checksum = res.pop().unwrap();
    let mut buf = vec![];
    for r in res {
        buf.append(&mut r.to_vec());
    }
    check_checksum(&buf, checksum);
    let mut file = std::fs::File::create(output).unwrap();
    file.write_all(&buf).unwrap();
    green_ln!("Done!");
}
fn check_checksum(buf: &[u8], checksum: &[u8]) {
    blue_ln!("Validating checksum");
    let crc = crc32fast::hash(buf);
    let crc = crc.to_be_bytes();
    if crc != checksum {
        e_red_ln!("Checksum mismatch");
        std::process::exit(1);
    }
    green_ln!("Checksum validated!");
}
fn encode(input: &str, output: &str, rows: usize, cols: usize, ec: EcLevel, version: Version) {
    blue_ln!(
        "Encoding {} into {} with {} rows and {} columns",
        input,
        output,
        rows,
        cols
    );
    if !std::fs::metadata(output).unwrap().is_dir() {
        e_red_ln!("Output is not a directory");
        std::process::exit(1);
    }
    let mut file = std::fs::File::open(input).unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();
    let size = qr_data::QR_DATA
        .into_iter()
        .find(|d| d.version == version && d.ec == ec)
        .unwrap()
        .bytes as usize;
    let data_split: Vec<Vec<u8>> = buffer
        .chunks(size - 4)
        .enumerate()
        .map(|(i, chunk)| {
            let b1 = i as u8;
            let b2 = (i >> 8) as u8;
            let mut data = vec![b2, b1];
            data.extend_from_slice(chunk);
            data
        })
        .collect();
    let mut tera = Tera::default();
    match tera.add_raw_template("p.svg", include_str!("../svg.tera")) {
        Ok(t) => t,
        Err(e) => {
            e_red_ln!("Error: {}", e);
            std::process::exit(1);
        }
    };
    struct Code {
        svg: String,
        label: String,
    }
    let mut codes = Vec::new();
    for (i, chunk) in data_split.iter().enumerate() {
        blue_ln!("Generating chunk {}/{}", i + 1, data_split.len());
        let code = QrCode::with_version(chunk, version, ec).unwrap();
        let image = code
            .render()
            .min_dimensions(200, 200)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .build();
        codes.push(Code {
            svg: image.to_string(),
            label: format!("Code {} of {}", i + 1, data_split.len()),
        });
    }
    let checksum = generate_checksum_code(&buffer, data_split.len());
    codes.push(Code {
        svg: checksum,
        label: "Checksum".to_string(),
    });
    for (p, page) in codes.chunks(rows * cols).enumerate() {
        let mut context = Context::new();
        let mut codes = Vec::new();
        blue_ln!(
            "Writing page {}/{}",
            p + 1,
            data_split.chunks(rows * cols).len()
        );
        for (i, chunk) in page.iter().enumerate() {
            let mut pc = Context::new();
            let base64 = base64::encode(chunk.svg.as_bytes());
            let r = i / cols;
            let c = i % cols;
            pc.insert("r", &r);
            pc.insert("c", &c);
            pc.insert("href", &format!("data:image/svg+xml;base64,{}", &base64));
            pc.insert("page", &p);
            pc.insert("label", &chunk.label);
            codes.push(pc.into_json());
        }
        context.insert("rows", &rows);
        context.insert("cols", &cols);
        context.insert("codes", &(codes));
        let result = Tera::one_off(include_str!("../svg.tera"), &context, false).unwrap();
        create_png(result.as_bytes(), &format!("{}/page{}.png", output, p));
    }
    green_ln!(
        "Done, wrote {} pages!",
        data_split.chunks(rows * cols).len()
    );
}
fn generate_checksum_code(data: &[u8], i: usize) -> String {
    blue_ln!("Generating checksum");
    let crc = crc32fast::hash(data);
    let b1 = i as u8;
    let b2 = (i >> 8) as u8;
    let mut data = vec![b2, b1];
    data.extend_from_slice(&crc.to_be_bytes());
    let code = QrCode::with_version(data, Version::Normal(1), EcLevel::H).unwrap();
    code.render()
        .min_dimensions(200, 200)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build()
}

use usvg_text_layout::{fontdb, TreeTextToPath};
fn create_png(svg_data: &[u8], out_file: &str) {
    let opt = usvg::Options::default();

    let mut fontdb = fontdb::Database::new();
    fontdb.load_system_fonts();

    let mut tree = usvg::Tree::from_data(svg_data, &opt).unwrap();
    tree.convert_text(&fontdb, opt.keep_named_groups);

    let pixmap_size = tree.size.to_screen_size();
    let dpi = 3;
    let mut pixmap =
        tiny_skia::Pixmap::new(pixmap_size.width() * dpi, pixmap_size.height() * dpi).unwrap();
    resvg::render(
        &tree,
        usvg::FitTo::Original,
        tiny_skia::Transform::from_scale(dpi as f32, dpi as f32),
        pixmap.as_mut(),
    )
    .unwrap();
    pixmap.save_png(out_file).unwrap();
}
