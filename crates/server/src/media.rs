//! Images for finds, zones and chapters: processing
//! (decode, turn upright, at most 1280 px, JPEG; drops EXIF/GPS), licences,
//! the approval rule, the media bucket (`ELEMENTS_MEDIA_S3_*`) and fetching a
//! picked Commons photo. Handlers: `editor_media.rs`.

use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

/// Longest side of a stored image.
pub const MAX_SIDE: u32 = 1280;
/// Largest file the editor may send.
pub const MAX_IN: usize = 8 << 20;
/// Aim for about this size; quality steps down until it fits.
const TARGET_BYTES: usize = 200_000;
const QUALITY: [u8; 4] = [82, 74, 66, 58];
/// Fallback long side when even the lowest quality is too big.
const SMALLER_SIDE: u32 = 1024;
pub const KINDS: [&str; 3] = ["find", "zone", "challenge"];
pub const CACHE: &str = "public, max-age=31536000, immutable";

/// A processed image, ready for the bucket.
pub struct Processed {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode JPEG/PNG/WebP, turn it upright (EXIF orientation), shrink to
/// [`MAX_SIDE`], flatten transparency on white and encode as JPEG. The output
/// carries no metadata (EXIF, GPS, camera).
pub fn process(input: &[u8]) -> Result<Processed, &'static str> {
    if input.len() > MAX_IN {
        return Err("image larger than 8 MB");
    }
    let mut r = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| "not an image")?;
    if !matches!(
        r.format(),
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    ) {
        return Err("JPEG, PNG or WebP only");
    }
    let mut lim = Limits::default();
    lim.max_image_width = Some(12_000);
    lim.max_image_height = Some(12_000);
    lim.max_alloc = Some(600 << 20);
    r.limits(lim);
    let mut dec = r.into_decoder().map_err(|_| "image unreadable")?;
    let orient = dec
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(dec).map_err(|_| "image unreadable")?;
    img.apply_orientation(orient);
    if img.width() < 200 || img.height() < 150 {
        return Err("image too small (at least 200 x 150 px)");
    }
    if img.width().max(img.height()) > MAX_SIDE {
        img = img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3);
    }
    let mut rgb = flatten(img);
    let mut jpeg = encode(&rgb)?;
    // Very detailed photos: a bit smaller rather than blocky.
    if jpeg.len() > TARGET_BYTES * 5 / 4 && rgb.width().max(rgb.height()) > SMALLER_SIDE {
        rgb = DynamicImage::ImageRgb8(rgb)
            .resize(
                SMALLER_SIDE,
                SMALLER_SIDE,
                image::imageops::FilterType::Lanczos3,
            )
            .into_rgb8();
        jpeg = encode(&rgb)?;
    }
    Ok(Processed {
        jpeg,
        width: rgb.width(),
        height: rgb.height(),
    })
}

/// JPEG at the first quality that fits [`TARGET_BYTES`] (else the lowest).
fn encode(rgb: &image::RgbImage) -> Result<Vec<u8>, &'static str> {
    let mut jpeg = Vec::new();
    for q in QUALITY {
        jpeg.clear();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, q)
            .encode_image(rgb)
            .map_err(|_| "encoding failed")?;
        if jpeg.len() <= TARGET_BYTES {
            break;
        }
    }
    Ok(jpeg)
}

/// RGB, transparent parts on white.
fn flatten(img: DynamicImage) -> image::RgbImage {
    if !img.color().has_alpha() {
        return img.into_rgb8();
    }
    let rgba = img.into_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        let c = |v: u8| ((v as u32 * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([c(p[0]), c(p[1]), c(p[2])])
    })
}

/// A licence as stored: code (`own`, `cc-by`, `cc-by-sa`, `cc0`, `pd`) and version.
#[derive(Debug, PartialEq, Clone)]
pub struct Licence {
    pub code: &'static str,
    pub version: Option<String>,
}

impl Licence {
    /// What the credit line shows (`None`: the city's own photo).
    pub fn label(&self) -> Option<String> {
        let v = self
            .version
            .as_deref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        match self.code {
            "cc-by" => Some(format!("CC BY{v}")),
            "cc-by-sa" => Some(format!("CC BY-SA{v}")),
            "cc0" => Some("CC0".into()),
            "pd" => Some("Public Domain".into()),
            _ => None,
        }
    }

    /// The licence text (CC only).
    pub fn url(&self) -> Option<String> {
        let v = self.version.as_deref()?;
        match self.code {
            "cc-by" => Some(format!("https://creativecommons.org/licenses/by/{v}/")),
            "cc-by-sa" => Some(format!("https://creativecommons.org/licenses/by-sa/{v}/")),
            "cc0" => Some(format!(
                "https://creativecommons.org/publicdomain/zero/{v}/"
            )),
            _ => None,
        }
    }
}

/// A licence the editor chose for an upload.
pub fn upload_licence(code: &str) -> Result<Licence, &'static str> {
    let (code, v): (&'static str, Option<&str>) = match code {
        "own" => ("own", None),
        "cc-by-4.0" => ("cc-by", Some("4.0")),
        "cc-by-sa-4.0" => ("cc-by-sa", Some("4.0")),
        "cc0-1.0" => ("cc0", Some("1.0")),
        _ => return Err("licence: own, cc-by-4.0, cc-by-sa-4.0 or cc0-1.0"),
    };
    Ok(Licence {
        code,
        version: v.map(Into::into),
    })
}

/// A Commons `LicenseShortName` ("CC BY-SA 4.0", "CC0", "Public domain"),
/// if free in our sense (no NC/ND, no GFDL-only).
pub fn commons_licence(short: &str) -> Option<Licence> {
    let s = short.trim().to_ascii_lowercase().replace('-', " ");
    let mut w = s.split_whitespace();
    let first = w.next()?;
    if first == "cc0" {
        return Some(Licence {
            code: "cc0",
            version: Some("1.0".into()),
        });
    }
    if first == "pd" || s == "public domain" || s.starts_with("public domain ") {
        return Some(Licence {
            code: "pd",
            version: None,
        });
    }
    if first != "cc" || w.next()? != "by" {
        return None;
    }
    let mut code = "cc-by";
    let mut version = None;
    for t in w {
        match t {
            "sa" if version.is_none() && code == "cc-by" => code = "cc-by-sa",
            v if version.is_none() && v.len() == 3 && v.as_bytes()[1] == b'.' => {
                version = Some(v.to_string())
            }
            // a port ("de", "at") after the version
            p if version.is_some()
                && (2..=3).contains(&p.len())
                && p.bytes().all(|b| b.is_ascii_lowercase()) => {}
            _ => return None,
        }
    }
    Some(Licence { code, version })
}

/// A second person decides: not the one who uploaded it.
pub fn may_decide(uploaded_by: &str, by: &str, master: bool) -> Result<(), &'static str> {
    if by.trim().is_empty() {
        return Err("no name on this token");
    }
    // The master token (`ELEMENTS_OPS_TOKEN`) approves directly.
    if by == uploaded_by && !master {
        return Err("a second person approves (not the uploader)");
    }
    Ok(())
}

/// Text of an HTML value (Commons `Artist`), one line, short.
pub fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                tag = true;
                out.push(' ');
            }
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ");
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out.chars().take(120).collect()
}

// --- The bucket -------------------------------------------------------------

/// The media bucket (`ELEMENTS_MEDIA_S3_ENDPOINT`, `_BUCKET`, `_KEY_ID`,
/// `_SECRET`, optional `_REGION`). Objects are public-read; the app loads
/// them from `ELEMENTS_MEDIA_PUBLIC_URL` (default: path style on the
/// endpoint). `ELEMENTS_MEDIA_PRIVATE=1`: the app goes through
/// `/api/media/<id>`, a redirect to a presigned URL.
/// `ELEMENTS_MEDIA_S3_PREFIX` (e.g. `dev/`): new files go under it, and this
/// server deletes only its own (after a reset of dev to prod, dev's database
/// points at prod's files: dev shows them but never deletes them).
pub struct Store {
    bucket: rusty_s3::Bucket,
    creds: rusty_s3::Credentials,
    agent: ureq::Agent,
    public: String,
    pub private: bool,
    prefix: String,
}

/// `key` belongs to the server with `prefix` (none: the region keys at the root).
fn owns(prefix: &str, key: &str) -> bool {
    if prefix.is_empty() {
        key.starts_with("region-")
    } else {
        key.starts_with(prefix)
    }
}

pub fn agent() -> Option<ureq::Agent> {
    let tls = native_tls::TlsConnector::new()
        .map_err(|e| tracing::warn!(error = %e, "media: no TLS"))
        .ok()?;
    Some(
        ureq::AgentBuilder::new()
            .tls_connector(Arc::new(tls))
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("glutflut-server (glutflut.com; tr@scientific-wave.com)")
            .build(),
    )
}

impl Store {
    fn from_env() -> Option<Self> {
        let var = |k: &str| {
            std::env::var(format!("ELEMENTS_MEDIA_S3_{k}"))
                .ok()
                .filter(|v| !v.is_empty())
        };
        let endpoint = url::Url::parse(&var("ENDPOINT")?).ok()?;
        let region = var("REGION").unwrap_or_else(|| {
            endpoint
                .host_str()
                .and_then(|h| h.strip_suffix(".your-objectstorage.com"))
                .unwrap_or("us-east-1")
                .to_string()
        });
        let name = var("BUCKET")?;
        let public = std::env::var("ELEMENTS_MEDIA_PUBLIC_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| format!("{}/{name}", endpoint.as_str().trim_end_matches('/')));
        let bucket =
            rusty_s3::Bucket::new(endpoint, rusty_s3::UrlStyle::Path, name, region).ok()?;
        Some(Self {
            bucket,
            creds: rusty_s3::Credentials::new(var("KEY_ID")?, var("SECRET")?),
            agent: agent()?,
            public: public.trim_end_matches('/').to_string(),
            private: std::env::var("ELEMENTS_MEDIA_PRIVATE").is_ok_and(|v| v == "1"),
            prefix: match var("PREFIX") {
                None => String::new(),
                Some(p) => {
                    let p = format!("{}/", p.trim_matches('/'));
                    if !p
                        .trim_end_matches('/')
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                        || p.starts_with("region-")
                    {
                        tracing::error!(prefix = %p, "media: ELEMENTS_MEDIA_S3_PREFIX: lowercase letters, digits, -; uploads off");
                        return None;
                    }
                    p
                }
            },
        })
    }

    /// The bucket key for `rest` (`<region>/<kind>/<file>`) under this server's prefix.
    pub fn key(&self, rest: &str) -> String {
        format!("{}{rest}", self.prefix)
    }

    pub fn put(&self, key: &str, body: &[u8]) -> Result<(), String> {
        use rusty_s3::S3Action;
        let url = self
            .bucket
            .put_object(Some(&self.creds), key)
            .sign(std::time::Duration::from_secs(300));
        self.agent
            .put(url.as_str())
            .set("Content-Type", "image/jpeg")
            .set("Cache-Control", CACHE)
            .send_bytes(body)
            .map(|_| ())
            .map_err(brief)
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        use rusty_s3::S3Action;
        if !owns(&self.prefix, key) {
            tracing::info!(key, "media: not this server's file (another prefix); kept");
            return Ok(());
        }
        let url = self
            .bucket
            .delete_object(Some(&self.creds), key)
            .sign(std::time::Duration::from_secs(300));
        self.agent
            .delete(url.as_str())
            .call()
            .map(|_| ())
            .map_err(brief)
    }

    /// Where the app loads `key` from (`id` for the private path).
    pub fn url(&self, key: &str, id: i64) -> String {
        if self.private {
            format!("/api/media/{id}")
        } else {
            format!("{}/{key}", self.public)
        }
    }

    /// A GET valid for an hour (private bucket).
    pub fn presigned(&self, key: &str) -> String {
        use rusty_s3::S3Action;
        self.bucket
            .get_object(Some(&self.creds), key)
            .sign(std::time::Duration::from_secs(3600))
            .to_string()
    }

    /// Scheme and host the app loads images from (the pages' CSP).
    pub fn origin(&self) -> Option<String> {
        let u = url::Url::parse(if self.private {
            self.bucket.base_url().as_str()
        } else {
            &self.public
        })
        .ok()?;
        Some(u.origin().ascii_serialization())
    }
}

/// The bucket, if configured.
pub fn store() -> Option<&'static Store> {
    static S: OnceLock<Option<Store>> = OnceLock::new();
    S.get_or_init(|| {
        let s = Store::from_env();
        if s.is_none() {
            tracing::info!("media: no ELEMENTS_MEDIA_S3_* (image uploads off)");
        }
        s
    })
    .as_ref()
}

/// An error without the request's URL (it carries the signature).
fn brief(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            format!(
                "HTTP {code}: {}",
                body.chars().take(200).collect::<String>()
            )
        }
        ureq::Error::Transport(t) => format!("{}: {}", t.kind(), t.message().unwrap_or("")),
    }
}

// --- Commons ----------------------------------------------------------------

/// A Commons photo picked in the editor, as fetched.
pub struct CommonsPhoto {
    pub bytes: Vec<u8>,
    pub author: String,
    pub licence: Licence,
    pub licence_url: Option<String>,
    pub page: String,
}

/// Fetch `File:…` at about [`MAX_SIDE`] px with author and licence from the
/// Commons API (blocking).
pub fn fetch_commons(title: &str) -> Result<CommonsPhoto, String> {
    if !title.starts_with("File:") || title.len() > 300 {
        return Err("not a Commons file".into());
    }
    let agent = agent().ok_or("no TLS")?;
    let r: serde_json::Value = agent
        .get("https://commons.wikimedia.org/w/api.php")
        .query("action", "query")
        .query("format", "json")
        .query("formatversion", "2")
        .query("prop", "imageinfo")
        .query("titles", title)
        .query("iiprop", "url|extmetadata|mime")
        .query("iiurlwidth", &MAX_SIDE.to_string())
        .query("iiextmetadatafilter", "Artist|LicenseShortName|LicenseUrl")
        .call()
        .map_err(brief)?
        .into_json()
        .map_err(|e| e.to_string())?;
    let ii = r
        .pointer("/query/pages/0/imageinfo/0")
        .ok_or("no such file on Commons")?;
    let s = |p: &str| ii.pointer(p).and_then(|v| v.as_str()).unwrap_or("");
    let licence = commons_licence(&plain(s("/extmetadata/LicenseShortName/value")))
        .ok_or("licence not free (CC0, CC BY, CC BY-SA, public domain)")?;
    let thumb = s("/thumburl");
    let host = url::Url::parse(thumb)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    if !["upload.wikimedia.org", "thumb.wikimedia.org"].contains(&host.as_str()) {
        return Err("unexpected image host".into());
    }
    let resp = agent.get(thumb).call().map_err(brief)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader().take(MAX_IN as u64 + 1), &mut bytes)
        .map_err(|e| e.to_string())?;
    let author = plain(s("/extmetadata/Artist/value"));
    Ok(CommonsPhoto {
        bytes,
        author: if author.is_empty() {
            "unbekannt".into()
        } else {
            author
        },
        licence_url: Some(s("/extmetadata/LicenseUrl/value"))
            .filter(|u| u.starts_with("http"))
            .map(String::from),
        licence,
        page: s("/descriptionurl").to_string(),
    })
}

use std::io::Read as _;

#[cfg(test)]
mod tests {
    #[test]
    fn a_server_deletes_only_its_own_files() {
        assert!(owns("", "region-konstanz/find/1-ab.jpg"));
        assert!(!owns("", "dev/region-konstanz/find/1-ab.jpg"));
        assert!(owns("dev/", "dev/region-konstanz/find/1-ab.jpg"));
        assert!(
            !owns("dev/", "region-konstanz/find/1-ab.jpg"),
            "prod's file, seen from dev after a reset"
        );
    }

    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                90,
                if x < 10 { 0 } else { 255 },
            ])
        });
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    /// A JPEG with an EXIF segment (orientation `o`, a fake GPS note) after SOI.
    fn jpeg_with_exif(w: u32, h: u32, o: u16) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x % 256) as u8, 40, 200]));
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 90)
            .encode_image(&img)
            .unwrap();
        // TIFF, big endian: one IFD entry, orientation.
        let mut tiff = b"MM\x00\x2a\x00\x00\x00\x08\x00\x01".to_vec();
        tiff.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1]);
        tiff.extend_from_slice(&o.to_be_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        tiff.extend_from_slice(b"GPS 47.66N 9.17E secret");
        let mut app1 = b"Exif\x00\x00".to_vec();
        app1.extend_from_slice(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&plain[2..]);
        out
    }

    fn has(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn shrinks_to_the_long_side() {
        let p = process(&png(3000, 1000)).unwrap();
        assert_eq!((p.width, p.height), (1280, 427));
        assert_eq!(&p.jpeg[..2], &[0xFF, 0xD8], "a JPEG");
        let small = process(&png(640, 480)).unwrap();
        assert_eq!((small.width, small.height), (640, 480), "never enlarged");
        assert!(process(&png(100, 80)).is_err(), "too small");
        assert!(process(b"GIF89a....").is_err());
    }

    #[test]
    fn drops_exif_and_turns_upright() {
        let src = jpeg_with_exif(400, 200, 6);
        assert!(has(&src, b"Exif") && has(&src, b"secret"));
        let p = process(&src).unwrap();
        assert!(!has(&p.jpeg, b"Exif"), "no EXIF");
        assert!(!has(&p.jpeg, b"secret"), "no GPS");
        assert_eq!((p.width, p.height), (200, 400), "orientation 6: turned");
        let p = process(&jpeg_with_exif(400, 200, 1)).unwrap();
        assert_eq!((p.width, p.height), (400, 200));
    }

    #[test]
    fn transparency_on_white() {
        let p = process(&png(300, 200)).unwrap();
        let img = image::load_from_memory(&p.jpeg).unwrap().into_rgb8();
        assert!(
            img.get_pixel(2, 100).0.iter().all(|&c| c > 230),
            "white where transparent"
        );
    }

    #[test]
    fn a_second_person_approves() {
        assert!(may_decide("Anna (Stadt)", "Ben (Stadt)", false).is_ok());
        assert!(may_decide("Anna (Stadt)", "Anna (Stadt)", false).is_err());
        assert!(may_decide("Anna (Stadt)", " ", false).is_err());
        assert!(
            may_decide("ops", "ops", true).is_ok(),
            "master: own ones too"
        );
    }

    #[test]
    fn licences() {
        let l = commons_licence("CC BY-SA 4.0").unwrap();
        assert_eq!((l.code, l.version.as_deref()), ("cc-by-sa", Some("4.0")));
        assert_eq!(l.label().unwrap(), "CC BY-SA 4.0");
        assert_eq!(
            l.url().unwrap(),
            "https://creativecommons.org/licenses/by-sa/4.0/"
        );
        assert_eq!(commons_licence("CC BY 2.0").unwrap().code, "cc-by");
        assert_eq!(
            commons_licence("CC BY-SA 3.0 de")
                .unwrap()
                .version
                .as_deref(),
            Some("3.0")
        );
        assert_eq!(commons_licence("CC0").unwrap().code, "cc0");
        assert_eq!(commons_licence("Public domain").unwrap().code, "pd");
        assert!(commons_licence("CC BY-NC 2.0").is_none());
        assert!(commons_licence("GFDL").is_none());
        assert!(commons_licence("CC BY-ND 4.0").is_none());
        assert_eq!(upload_licence("own").unwrap().label(), None);
        assert!(upload_licence("cc-by-nc").is_err());
        assert_eq!(
            plain("<a href=\"//x\">Jo &amp; Bo</a>  (talk)"),
            "Jo & Bo (talk)"
        );
    }
}
