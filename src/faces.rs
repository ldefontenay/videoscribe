//! Finding faces in still pictures and making a fingerprint of each, on this
//! computer. Nothing here is sent anywhere (PLAN.md §5.7, milestone 10).
//!
//! Photo Scribe's models and code (`faces.rs`, MIT, same author): YuNet
//! finds faces (MIT licence), SFace turns each face into 128 numbers that
//! are similar for the same person (Apache 2.0 licence). Both come from the
//! OpenCV model zoo, are downloaded once and checked against their SHA-256,
//! and run through tract, which Video Scribe already has for Silero.
//!
//! Photo Scribe's own copies of the models are used when it has them (log
//! entry 42, Q4); otherwise Video Scribe downloads its own, once, after
//! asking.
//!
//! `tag` finds the faces in a video's pictures and gives each person a tag,
//! "Person 3", the same all through the video (log entry 44).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use tract_onnx::prelude::*;

use crate::frames::Pixels;
use crate::net;

type Plan = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

pub const EMBEDDING: usize = 128;

struct ModelFile {
    file: &'static str,
    path: &'static str,
    size: u64,
    sha256: &'static str,
}

const HOST: &str = "media.githubusercontent.com";
const YUNET: ModelFile = ModelFile {
    file: "face_detection_yunet_2023mar.onnx",
    path: "/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
    size: 232_589,
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
};
const SFACE: ModelFile = ModelFile {
    file: "face_recognition_sface_2021dec.onnx",
    path: "/media/opencv/opencv_zoo/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx",
    size: 38_696_353,
    sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
};

const DETECT_SIZE: usize = 640;
const SCORE_MIN: f32 = 0.7;
const NMS_IOU: f32 = 0.3;
/// Faces smaller than this, in pixels, are too small to recognise reliably.
const MIN_FACE: f32 = 24.0;
/// Photo Scribe's: unnamed faces join a group this similar to its average.
pub const CLUSTER: f32 = 0.40;
/// Where the five landmarks sit in SFace's 112x112 input (the ArcFace template).
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// A face as found: where it is, how sure the finder is, and its fingerprint.
#[derive(Clone, Debug)]
pub struct Found {
    /// x, y, width, height, as fractions of the picture's width and height.
    pub bbox: [f32; 4],
    pub score: f32,
    /// Unit length, so the dot product of two is their similarity.
    pub embedding: Vec<f32>,
}

pub struct Models {
    yunet: Plan,
    outputs: Vec<String>,
    sface: Plan,
}

/// Video Scribe's own copies, downloaded when Photo Scribe has none.
fn own_models_dir() -> PathBuf {
    crate::ytdlp::local_dir().join("models")
}

/// Photo Scribe's settings folder, `%APPDATA%\Photo Scribe`.
pub fn photo_scribe_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Photo Scribe"))
}

fn complete(dir: &Path) -> bool {
    [YUNET, SFACE]
        .iter()
        .all(|m| std::fs::metadata(dir.join(m.file)).is_ok_and(|md| md.len() == m.size))
}

/// Where the models are: Photo Scribe's copies, the same public files, when
/// it has both; else Video Scribe's own.
fn models_dir() -> PathBuf {
    photo_scribe_dir()
        .map(|d| d.join("models"))
        .filter(|d| complete(d))
        .unwrap_or_else(own_models_dir)
}

pub fn models_present() -> bool {
    complete(&models_dir())
}

/// Downloads whichever model is missing, checking each against its published
/// SHA-256 before keeping it. False if stopped.
pub fn download_models(
    stop: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<bool, String> {
    let dir = own_models_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("The folder for the face models could not be made: {e}"))?;
    for m in [YUNET, SFACE] {
        let target = dir.join(m.file);
        if std::fs::metadata(&target).is_ok_and(|md| md.len() == m.size) {
            continue;
        }
        let Some(data) = net::download(HOST, m.path, stop, progress)? else {
            return Ok(false);
        };
        if data.len() as u64 != m.size || net::hex(&net::sha256(&data)) != m.sha256 {
            return Err(format!(
                "The face model downloaded from GitHub ({}) is not the one Video Scribe expects. \
                 It may have been changed by the OpenCV project. Nothing was kept.",
                m.file
            ));
        }
        let tmp = target.with_extension("part");
        std::fs::write(&tmp, &data)
            .and_then(|_| std::fs::rename(&tmp, &target))
            .map_err(|e| format!("The face model could not be saved: {e}"))?;
    }
    Ok(true)
}

/// The models, loaded from their folder.
pub fn load() -> Result<Models, String> {
    let dir = models_dir();
    let fail = |e: TractError| format!("The face models could not be loaded: {e}");
    let detector = tract_onnx::onnx()
        .model_for_path(dir.join(YUNET.file))
        .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, DETECT_SIZE, DETECT_SIZE]).into()))
        .map_err(fail)?;
    let outputs = detector
        .output_outlets()
        .map_err(fail)?
        .iter()
        .map(|o| {
            detector
                .outlet_label(*o)
                .map(str::to_string)
                .unwrap_or_else(|| detector.node(o.node).name.clone())
        })
        .collect();
    let yunet = detector
        .into_optimized()
        .and_then(|m| m.into_runnable())
        .map_err(fail)?;
    let sface = tract_onnx::onnx()
        .model_for_path(dir.join(SFACE.file))
        .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, 112, 112]).into()))
        .and_then(|m| m.into_optimized())
        .and_then(|m| m.into_runnable())
        .map_err(fail)?;
    Ok(Models {
        yunet,
        outputs,
        sface,
    })
}

/// Every face in the picture, largest first.
pub fn find(models: &Models, img: &Pixels) -> Result<Vec<Found>, String> {
    let fail = |e: TractError| format!("Finding faces failed: {e}");
    let detected = detect(models, img).map_err(fail)?;
    let (w, h) = (img.width as f32, img.height as f32);
    let mut out = Vec::new();
    for (score, bbox, kps) in detected {
        if bbox[2].min(bbox[3]) < MIN_FACE {
            continue;
        }
        let face = align(img, &kps);
        out.push(Found {
            bbox: [bbox[0] / w, bbox[1] / h, bbox[2] / w, bbox[3] / h],
            score,
            embedding: embed(models, &face).map_err(fail)?,
        });
    }
    out.sort_by(|a, b| (b.bbox[2] * b.bbox[3]).total_cmp(&(a.bbox[2] * a.bbox[3])));
    Ok(out)
}

type Detection = (f32, [f32; 4], [[f32; 2]; 5]);

fn detect(models: &Models, img: &Pixels) -> TractResult<Vec<Detection>> {
    let (w, h) = (img.width as usize, img.height as usize);
    let scale = DETECT_SIZE as f32 / w.max(h) as f32;
    let (rw, rh) = (
        ((w as f32 * scale).round() as usize).clamp(1, DETECT_SIZE),
        ((h as f32 * scale).round() as usize).clamp(1, DETECT_SIZE),
    );
    let n = DETECT_SIZE;
    // BGR, 0..255, zero padding at the right and bottom: what OpenCV's
    // FaceDetectorYN feeds it. Box filter, or nearest when enlarging.
    let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, n, n));
    for y in 0..rh {
        let (y0, y1) = (y * h / rh, ((y + 1) * h / rh).max(y * h / rh + 1).min(h));
        for x in 0..rw {
            let (x0, x1) = (x * w / rw, ((x + 1) * w / rw).max(x * w / rw + 1).min(w));
            let mut sum = [0u32; 3];
            for yy in y0..y1 {
                let row = &img.bgr[(yy * w + x0) * 3..(yy * w + x1) * 3];
                for px in row.as_chunks::<3>().0 {
                    sum[0] += px[0] as u32;
                    sum[1] += px[1] as u32;
                    sum[2] += px[2] as u32;
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as f32;
            for c in 0..3 {
                input[[0, c, y, x]] = sum[c] as f32 / count;
            }
        }
    }
    let out = models.yunet.run(tvec!(Tensor::from(input).into()))?;
    let get = |name: &str| -> TractResult<&[f32]> {
        let i = models
            .outputs
            .iter()
            .position(|o| o == name)
            .ok_or_else(|| TractError::msg(format!("the face finder has no output {name}")))?;
        out[i].as_slice::<f32>()
    };
    let mut found = Vec::new();
    for stride in [8usize, 16, 32] {
        let cls = get(&format!("cls_{stride}"))?;
        let obj = get(&format!("obj_{stride}"))?;
        let bbox = get(&format!("bbox_{stride}"))?;
        let kps = get(&format!("kps_{stride}"))?;
        let cols = n / stride;
        for idx in 0..cls.len() {
            let score = (cls[idx].clamp(0.0, 1.0) * obj[idx].clamp(0.0, 1.0)).sqrt();
            if score < SCORE_MIN {
                continue;
            }
            let (r, c) = ((idx / cols) as f32, (idx % cols) as f32);
            let s = stride as f32;
            let b = &bbox[idx * 4..idx * 4 + 4];
            let (cx, cy) = ((c + b[0]) * s, (r + b[1]) * s);
            let (bw, bh) = (b[2].exp() * s, b[3].exp() * s);
            let k = &kps[idx * 10..idx * 10 + 10];
            let mut pts = [[0.0f32; 2]; 5];
            for (j, p) in pts.iter_mut().enumerate() {
                *p = [(k[2 * j] + c) * s / scale, (k[2 * j + 1] + r) * s / scale];
            }
            found.push((
                score,
                [
                    (cx - bw / 2.0) / scale,
                    (cy - bh / 2.0) / scale,
                    bw / scale,
                    bh / scale,
                ],
                pts,
            ));
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut kept: Vec<Detection> = Vec::new();
    for f in found {
        if kept.iter().all(|k| iou(&k.1, &f.1) < NMS_IOU) {
            kept.push(f);
        }
    }
    Ok(kept)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    inter / (a[2] * a[3] + b[2] * b[3] - inter)
}

/// The face turned upright and scaled onto SFace's 112x112 template: the
/// least-squares similarity transform from the five landmarks, then bilinear
/// sampling. Returns RGB, which is what SFace expects.
fn align(img: &Pixels, kps: &[[f32; 2]; 5]) -> Vec<u8> {
    let n = 5.0;
    let ms = kps
        .iter()
        .fold([0.0, 0.0], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
    let md = TEMPLATE
        .iter()
        .fold([0.0, 0.0], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
    let (mut sa, mut sb, mut ss) = (0.0f32, 0.0f32, 0.0f32);
    for (s, d) in kps.iter().zip(TEMPLATE.iter()) {
        let (xs, ys) = (s[0] - ms[0], s[1] - ms[1]);
        let (xd, yd) = (d[0] - md[0], d[1] - md[1]);
        sa += xs * xd + ys * yd;
        sb += xs * yd - ys * xd;
        ss += xs * xs + ys * ys;
    }
    let (a, b) = (sa / ss.max(1e-6), sb / ss.max(1e-6));
    let t = [
        md[0] - (a * ms[0] - b * ms[1]),
        md[1] - (b * ms[0] + a * ms[1]),
    ];
    let det = (a * a + b * b).max(1e-12);
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = vec![0u8; 112 * 112 * 3];
    for v in 0..112 {
        for u in 0..112 {
            let (du, dv) = (u as f32 - t[0], v as f32 - t[1]);
            let x = (a * du + b * dv) / det;
            let y = (-b * du + a * dv) / det;
            if x < 0.0 || y < 0.0 || x >= (w - 1) as f32 || y >= (h - 1) as f32 {
                continue;
            }
            let (x0, y0) = (x as usize, y as usize);
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            for c in 0..3 {
                let p = |xx: usize, yy: usize| img.bgr[(yy * w + xx) * 3 + c] as f32;
                let top = p(x0, y0) * (1.0 - fx) + p(x0 + 1, y0) * fx;
                let bot = p(x0, y0 + 1) * (1.0 - fx) + p(x0 + 1, y0 + 1) * fx;
                // BGR in, RGB out.
                out[(v * 112 + u) * 3 + (2 - c)] = (top * (1.0 - fy) + bot * fy).round() as u8;
            }
        }
    }
    out
}

/// The fingerprint of the face and of its mirror image, added together,
/// which is steadier against the way a head is turned (Photo Scribe's round 5).
fn embed(models: &Models, rgb: &[u8]) -> TractResult<Vec<f32>> {
    let mut sum = vec![0.0f32; EMBEDDING];
    for mirrored in [false, true] {
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, 112, 112), |(_, c, y, x)| {
            let x = if mirrored { 111 - x } else { x };
            rgb[(y * 112 + x) * 3 + c] as f32
        });
        let out = models.sface.run(tvec!(Tensor::from(input).into()))?;
        let v = out[0].as_slice::<f32>()?;
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        for (s, x) in sum.iter_mut().zip(v) {
            *s += x / norm;
        }
    }
    let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    Ok(sum.iter().map(|x| x / norm).collect())
}

/// How big the pictures faces are looked for in are, along their longer
/// side: as in the spike on Tears of Steel (log entry 42).
const LONG_EDGE: u32 = 1280;
/// Fingerprints kept for each person in a video, the clearest first: for
/// knowing them in later videos, and plenty for that.
const KEEP: usize = 20;

/// A face in a picture: its tag's number, and where it is.
pub type TaggedFace = (usize, [f32; 4]);

/// The faces found in a video's pictures, each with its person's tag.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tagged {
    /// Each picture's time, and its faces: the tag's number and where the
    /// face is (x, y, width, height, as fractions of the picture).
    pub pictures: Vec<(f64, Vec<TaggedFace>)>,
    /// Each tag's fingerprints, the clearest first, at most `KEEP`; tag n
    /// is at n - 1.
    pub people: Vec<Vec<Vec<f32>>>,
}

/// Where in the picture, in words a describer can use.
fn side(bbox: &[f32; 4]) -> &'static str {
    let x = bbox[0] + bbox[2] / 2.0;
    if x < 0.36 {
        "left"
    } else if x > 0.64 {
        "right"
    } else {
        "middle"
    }
}

pub fn tag_name(n: usize) -> String {
    format!("Person {n}")
}

/// The number in "Person 3", or None.
pub fn tag_number(tag: &str) -> Option<usize> {
    tag.trim().strip_prefix("Person ")?.trim().parse().ok()
}

impl Tagged {
    /// What the describer is told about a picture's faces: "Faces: Person 1
    /// (left), Person 3 (middle)." None when it has none.
    pub fn line_at(&self, t: f64) -> Option<String> {
        let (_, faces) = self.pictures.iter().find(|(pt, _)| (pt - t).abs() < 0.01)?;
        if faces.is_empty() {
            return None;
        }
        let mut faces = faces.clone();
        faces.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]));
        let list: Vec<String> = faces
            .iter()
            .map(|(n, b)| format!("{} ({})", tag_name(*n), side(b)))
            .collect();
        Some(format!("Faces: {}.", list.join(", ")))
    }

    /// The tags seen between two times, for a clip: "Faces: Person 1,
    /// Person 3." None when there are none.
    pub fn line_between(&self, start: f64, end: f64) -> Option<String> {
        let mut tags: Vec<usize> = self
            .pictures
            .iter()
            .filter(|(t, _)| *t >= start - 0.01 && *t <= end + 0.01)
            .flat_map(|(_, f)| f.iter().map(|(n, _)| *n))
            .collect();
        tags.sort();
        tags.dedup();
        if tags.is_empty() {
            return None;
        }
        let list: Vec<String> = tags.into_iter().map(tag_name).collect();
        Some(format!("Faces: {}.", list.join(", ")))
    }

    pub fn has(&self, times: &[f64]) -> bool {
        times
            .iter()
            .all(|t| self.pictures.iter().any(|(pt, _)| (pt - t).abs() < 0.01))
    }

    fn to_json(&self) -> Value {
        json!({
            "pictures": self.pictures.iter().map(|(t, f)| json!({
                "t": t,
                "faces": f.iter().map(|(n, b)| json!({"tag": n, "box": b})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "people": self.people,
        })
    }

    fn from_json(v: &Value) -> Option<Tagged> {
        let pictures = v["pictures"]
            .as_array()?
            .iter()
            .map(|p| {
                let faces = p["faces"]
                    .as_array()?
                    .iter()
                    .map(|f| {
                        let b: Vec<f32> = f["box"]
                            .as_array()?
                            .iter()
                            .map(|x| x.as_f64().map(|x| x as f32))
                            .collect::<Option<_>>()?;
                        Some((f["tag"].as_u64()? as usize, b.try_into().ok()?))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some((p["t"].as_f64()?, faces))
            })
            .collect::<Option<Vec<_>>>()?;
        let people = serde_json::from_value(v["people"].clone()).ok()?;
        Some(Tagged { pictures, people })
    }
}

/// Where a video's faces are kept, by its key: fingerprints, so on this
/// computer only, beside the checkpoints.
fn tagged_path(key: &str) -> PathBuf {
    crate::ytdlp::local_dir()
        .join("faces")
        .join(format!("{key}.json"))
}

/// The faces found in a video before, if they were.
pub fn tagged(key: &str) -> Option<Tagged> {
    let text = std::fs::read_to_string(tagged_path(key)).ok()?;
    Tagged::from_json(&serde_json::from_str(&text).ok()?)
}

fn keep_tagged(key: &str, t: &Tagged) -> Result<(), String> {
    let path = tagged_path(key);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = path.with_extension("part");
    std::fs::write(&part, t.to_json().to_string())
        .and_then(|_| std::fs::rename(&part, &path))
        .map_err(|e| format!("The faces found could not be kept: {e}"))
}

/// Finds the faces at each of `times`, groups them by who they look like,
/// and tags each group "Person 1", "Person 2"... in the order they first
/// appear. Kept by the video's key, so describing it again finds them at
/// once. None if stopped.
pub fn tag(
    key: &str,
    frames: &crate::frames::Frames,
    times: &[f64],
    stop: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Option<Tagged>, String> {
    if let Some(t) = tagged(key).filter(|t| t.has(times)) {
        return Ok(Some(t));
    }
    let models = load()?;
    let mut found: Vec<(f64, Vec<Found>)> = Vec::new();
    for (i, &t) in times.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        progress(i, times.len());
        // A picture that cannot be read has no faces, rather than failing
        // the describing: it is not sent either way.
        let faces = match frames.pixels(t, LONG_EDGE) {
            Ok(px) => find(&models, &px)?,
            Err(_) => Vec::new(),
        };
        found.push((t, faces));
    }
    let flat: Vec<(usize, &Found)> = found
        .iter()
        .enumerate()
        .flat_map(|(i, (_, f))| f.iter().map(move |x| (i, x)))
        .collect();
    let mut groups = group(&flat);
    // Numbered in the order they first appear.
    groups.sort_by_key(|g| g.iter().map(|&j| flat[j].0).min().unwrap_or(0));
    let mut tag_of = vec![0usize; flat.len()];
    for (g, members) in groups.iter().enumerate() {
        for &j in members {
            tag_of[j] = g + 1;
        }
    }
    let mut out = Tagged {
        pictures: found.iter().map(|(t, _)| (*t, Vec::new())).collect(),
        people: Vec::new(),
    };
    for (j, (i, f)) in flat.iter().enumerate() {
        out.pictures[*i].1.push((tag_of[j], f.bbox));
    }
    for members in &groups {
        let mut m = members.clone();
        let clarity = |j: usize| flat[j].1.score * flat[j].1.bbox[2] * flat[j].1.bbox[3];
        m.sort_by(|&a, &b| clarity(b).total_cmp(&clarity(a)));
        out.people.push(
            m.iter()
                .take(KEEP)
                .map(|&j| flat[j].1.embedding.clone())
                .collect(),
        );
    }
    keep_tagged(key, &out)?;
    Ok(Some(out))
}

pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Faces grouped by who they look like: greedy, each joining the group whose
/// average it is nearest, at least `CLUSTER` similar, clearest faces first.
/// Two faces from one picture never share a group. Indexes into `faces`,
/// whose first element says which picture each came from.
pub fn group(faces: &[(usize, &Found)]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..faces.len()).collect();
    let clarity = |i: usize| faces[i].1.score * faces[i].1.bbox[2] * faces[i].1.bbox[3];
    order.sort_by(|&a, &b| clarity(b).total_cmp(&clarity(a)));
    let mut sums: Vec<Vec<f32>> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let e = &faces[i].1.embedding;
        let best = sums
            .iter()
            .enumerate()
            .filter(|(g, _)| groups[*g].iter().all(|&j| faces[j].0 != faces[i].0))
            .map(|(g, s)| {
                let norm = s.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
                (g, similarity(e, s) / norm)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|&(_, s)| s >= CLUSTER);
        match best {
            Some((g, _)) => {
                sums[g].iter_mut().zip(e).for_each(|(s, x)| *s += x);
                groups[g].push(i);
            }
            None => {
                sums.push(e.clone());
                groups.push(vec![i]);
            }
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The face's box, a little larger, as a 64x64 BGR square.
    fn crop(px: &Pixels, bbox: &[f32; 4]) -> Vec<u8> {
        let (w, h) = (px.width as f32, px.height as f32);
        let side = (bbox[2] * w).max(bbox[3] * h) * 1.2;
        let (cx, cy) = ((bbox[0] + bbox[2] / 2.0) * w, (bbox[1] + bbox[3] / 2.0) * h);
        let mut out = vec![0u8; 64 * 64 * 3];
        for y in 0..64 {
            for x in 0..64 {
                let sx = (cx - side / 2.0 + side * x as f32 / 64.0).clamp(0.0, w - 1.0) as usize;
                let sy = (cy - side / 2.0 + side * y as f32 / 64.0).clamp(0.0, h - 1.0) as usize;
                let from = (sy * px.width as usize + sx) * 3;
                out[(y * 64 + x) * 3..(y * 64 + x) * 3 + 3]
                    .copy_from_slice(&px.bgr[from..from + 3]);
            }
        }
        out
    }

    /// A 24-bit BMP file, rows bottom first.
    fn bmp(w: usize, h: usize, bgr: &[u8]) -> Vec<u8> {
        let row = (w * 3).div_ceil(4) * 4;
        let size = 54 + row * h;
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes());
        out.extend_from_slice(&[1, 0, 24, 0]);
        out.extend_from_slice(&[0u8; 24]);
        for y in (0..h).rev() {
            out.extend_from_slice(&bgr[y * w * 3..(y + 1) * w * 3]);
            out.resize(out.len() + row - w * 3, 0);
        }
        out
    }

    /// The spike for milestone 10 (log entry 42): faces in pictures taken
    /// every few seconds of a film, grouped by who they look like. Run with
    /// VS_SPIKE_VIDEO set to a video, and VS_SPIKE_EVERY to seconds apart:
    /// `cargo test --release grouping_a_film -- --ignored --nocapture`.
    /// Downloads the models (39 MB) the first time.
    #[test]
    #[ignore]
    fn grouping_a_film() {
        let video = std::env::var("VS_SPIKE_VIDEO").expect("VS_SPIKE_VIDEO");
        let every: f64 = std::env::var("VS_SPIKE_EVERY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4.0);
        let edge: u32 = std::env::var("VS_SPIKE_EDGE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1280);
        let stop = AtomicBool::new(false);
        if !models_present() {
            assert!(download_models(&stop, &mut |_, _| {}).unwrap());
        }
        let t0 = std::time::Instant::now();
        let models = load().unwrap();
        println!("models loaded in {:.1} s", t0.elapsed().as_secs_f64());
        let _session = crate::media::Session::start().unwrap();
        // A YouTube picture file, as the job opens one (ytdlp::drop_edit_list).
        let shift = crate::ytdlp::drop_edit_list(std::path::Path::new(&video)).unwrap_or(0.0);
        let frames = crate::frames::Frames::open(&video, shift).unwrap();
        let mut found: Vec<(usize, f64, Found)> = Vec::new();
        let mut crops: Vec<Vec<u8>> = Vec::new();
        let (mut t, mut pictures, mut took) = (1.0, 0usize, 0.0);
        let mut size = (0, 0);
        loop {
            let px = match frames.pixels(t, edge) {
                Ok(px) => px,
                Err(why) => {
                    println!("stopped at {t}: {why}");
                    break;
                }
            };
            size = (px.width, px.height);
            let s = std::time::Instant::now();
            let faces = find(&models, &px).unwrap();
            took += s.elapsed().as_secs_f64();
            for f in faces {
                crops.push(crop(&px, &f.bbox));
                found.push((pictures, t, f));
            }
            pictures += 1;
            t += every;
        }
        println!(
            "{pictures} pictures of {}x{}, {} faces, {:.2} s a picture to find and fingerprint",
            size.0,
            size.1,
            found.len(),
            took / pictures.max(1) as f64
        );
        let refs: Vec<(usize, &Found)> = found.iter().map(|(p, _, f)| (*p, f)).collect();
        let groups = group(&refs);
        for (n, g) in groups.iter().enumerate().take(15) {
            let times: Vec<String> = g
                .iter()
                .take(12)
                .map(|&i| crate::speech::clock(found[i].1))
                .collect();
            let height: f32 = g.iter().map(|&i| found[i].2.bbox[3]).sum::<f32>() / g.len() as f32;
            println!(
                "group {}: {} faces, average height {:.0}% of the picture, at {}",
                n + 1,
                g.len(),
                height * 100.0,
                times.join(", ")
            );
        }
        // A contact sheet: a row of faces for each of the first groups.
        if let Ok(sheet) = std::env::var("VS_SPIKE_SHEET") {
            let (rows, cols, c) = (10usize, 10usize, 64usize);
            let (w, h) = (cols * c, rows * c);
            let mut bgr = vec![255u8; w * h * 3];
            for (r, g) in groups.iter().take(rows).enumerate() {
                for (k, &i) in g.iter().take(cols).enumerate() {
                    for y in 0..c {
                        for x in 0..c {
                            let at = ((r * c + y) * w + k * c + x) * 3;
                            let from = (y * c + x) * 3;
                            bgr[at..at + 3].copy_from_slice(&crops[i][from..from + 3]);
                        }
                    }
                }
            }
            std::fs::write(&sheet, bmp(w, h, &bgr)).unwrap();
        }
        println!(
            "{} groups in all, {} of a single face",
            groups.len(),
            groups.iter().filter(|g| g.len() == 1).count()
        );
    }

    #[test]
    fn tags_are_said_left_to_right_and_kept() {
        let t = Tagged {
            pictures: vec![
                (
                    1.5,
                    vec![(3, [0.7, 0.1, 0.1, 0.2]), (1, [0.1, 0.1, 0.1, 0.2])],
                ),
                (2.5, vec![(2, [0.45, 0.1, 0.1, 0.2])]),
                (3.5, vec![]),
            ],
            people: vec![vec![vec![0.5; EMBEDDING]], vec![], vec![]],
        };
        assert_eq!(
            t.line_at(1.5).as_deref(),
            Some("Faces: Person 1 (left), Person 3 (right).")
        );
        assert_eq!(t.line_at(2.5).as_deref(), Some("Faces: Person 2 (middle)."));
        assert_eq!(t.line_at(3.5), None);
        assert_eq!(
            t.line_between(1.0, 3.0).as_deref(),
            Some("Faces: Person 1, Person 2, Person 3.")
        );
        assert!(t.has(&[1.5, 3.5]));
        assert!(!t.has(&[1.5, 4.5]));
        assert_eq!(Tagged::from_json(&t.to_json()), Some(t));
        assert_eq!(tag_number("Person 12"), Some(12));
        assert_eq!(tag_number("the groom"), None);
    }
}
