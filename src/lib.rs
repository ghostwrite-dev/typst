//! Typst as a plugin. It keeps one file's world alive (its project's files, packages,
//! fonts), so each compile after the first only redoes what changed. The file's text is
//! what's on screen, saved or not; everything else it uses is read through what the
//! editor mounts for it, or for a package from Typst's own package folders
//! (downloading it the first time, by way of the editor).
//!
//! Each page says where its glyphs are and which bytes of the file they came from,
//! so that the editor can draw the cursor on the page and go from a click to the text.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ghostwrite_plugin_api::{Drawing, Glyph, Page, Plugin, Span, register};
use typst::diag::{FileError, FileResult, PackageError, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::layout::{Abs, Frame, FrameItem, Point, Transform};
use typst::syntax::package::PackageSpec;
use typst::syntax::{FileId, LinkedNode, RootedPath, Source, Tag, VirtualPath, VirtualRoot, highlight as tag_of};
use typst::text::{Font, FontBook, FontInfo};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
use typst_kit::datetime::Time;
use typst_kit::downloader::Downloader;
use typst_kit::files::{FileLoader, FileStore, FsRoot, SystemFiles};
use typst_kit::fonts::{FontPath, FontStore};
use typst_kit::packages::{FsPackages, SystemPackages, UniversePackages};
use typst_layout::PagedDocument;

/// The most pixels a page is drawn with: one shaped to be bigger isn't drawn at all.
const MAX_PIXELS: usize = 64 << 20;
/// What fonts were found to be, last time, so they needn't all be read each time.
const FONTS_KNOWN: &str = "/cache/fonts.json";

/// The standard library and the fonts: the system's and Typst's own, found once.
struct Env {
    library: LazyHash<Library>,
    fonts: FontStore,
}

fn env() -> &'static Env {
    static ENV: OnceLock<Env> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut fonts = FontStore::new();
        fonts.extend(system_fonts());
        fonts.extend(typst_kit::fonts::embedded());
        Env { library: LazyHash::new(Library::default()), fonts }
    })
}

fn font_files(folder: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            font_files(&path, found);
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| ["ttf", "otf", "ttc", "otc"].contains(&e.to_ascii_lowercase().as_str())) {
            found.push(path);
        }
    }
}

/// The fonts in the folders mounted. Reading one takes reading all of it, so what
/// each is is kept in the plugin's cache, by its name, size and when it last changed.
fn system_fonts() -> Vec<(FontPath, FontInfo)> {
    let mut files = vec![];
    // Each folder is mounted by itself, one after another: there's no /fonts to list.
    for folder in (0..).map(|ix| PathBuf::from(format!("/fonts/{ix}"))).take_while(|folder| folder.is_dir()) {
        font_files(&folder, &mut files);
    }
    files.sort();
    let mut known: HashMap<String, Vec<FontInfo>> = std::fs::read(FONTS_KNOWN).ok().and_then(|text| serde_json::from_slice(&text).ok()).unwrap_or_default();
    let mut found = vec![];
    let mut now = HashMap::new();
    let mut changed = false;
    for path in files {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let when = meta.modified().ok().and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |when| when.as_secs());
        let key = format!("{}|{}|{when}", path.display(), meta.len());
        let infos = known.remove(&key).unwrap_or_else(|| {
            changed = true;
            std::fs::read(&path).map_or(vec![], |bytes| FontInfo::iter(&bytes).collect())
        });
        for (index, info) in infos.iter().enumerate() {
            found.push((FontPath { path: path.clone(), index: index as u32 }, info.clone()));
        }
        now.insert(key, infos);
    }
    if changed && let Ok(text) = serde_json::to_vec(&now) {
        let _ = std::fs::write(FONTS_KNOWN, text);
    }
    found
}

/// Where Typst keeps packages, as the editor mounts them: what you've put there
/// yourself, and what's been downloaded, which is shared with Typst's own tools.
const YOUR_PACKAGES: &str = "/data/typst/packages";
const DOWNLOADED_PACKAGES: &str = "/user-cache/typst/packages";

/// Have the package `spec` where it'll be looked for, downloading it if it's from Typst
/// Universe and isn't there. Typst's own way of unpacking one needs to know a folder's
/// real path, which there isn't in here.
fn have_package(spec: &PackageSpec) -> Result<(), PackageError> {
    let within = format!("{}/{}/{}", spec.namespace, spec.name, spec.version);
    let there = |root: &str| Path::new(root).join(&within).is_dir();
    if there(YOUR_PACKAGES) || there(DOWNLOADED_PACKAGES) || spec.namespace != "preview" {
        return Ok(());
    }
    let url = format!("https://packages.typst.org/preview/{}-{}.tar.gz", spec.name, spec.version);
    let bytes = ghostwrite_plugin_api::fetch(&url).map_err(|why| PackageError::NetworkFailed(Some(why.into())))?;
    let wrong = |why: io::Error| PackageError::MalformedArchive(Some(why.to_string().into()));
    let folder = Path::new(DOWNLOADED_PACKAGES).join(&within);
    let unpacking = folder.with_file_name(format!(".unpacking-{}", spec.version));
    let _ = std::fs::remove_dir_all(&unpacking);
    let unpacked = unpack(&bytes, &unpacking).and_then(|()| std::fs::rename(&unpacking, &folder));
    if unpacked.is_err() {
        let _ = std::fs::remove_dir_all(&unpacking);
    }
    unpacked.map_err(wrong)
}

/// The files of a `.tar.gz` under `folder`, and none outside it, whatever it says.
fn unpack(bytes: &[u8], folder: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    std::fs::create_dir_all(folder)?;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if !path.components().all(|part| matches!(part, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
            return Err(io::Error::other(format!("{} is outside the package", path.display())));
        }
        let to = folder.join(&path);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&to)?;
        } else if kind.is_file() {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut contents = vec![];
            entry.read_to_end(&mut contents)?;
            std::fs::write(&to, contents)?;
        }
    }
    Ok(())
}

/// Downloads by asking the editor, which holds it to what the plugin says it uses.
struct ByTheEditor;

impl Downloader for ByTheEditor {
    fn stream(&self, _key: &dyn Any, url: &str) -> io::Result<(Option<usize>, Box<dyn Read>)> {
        let bytes = ghostwrite_plugin_api::fetch(url).map_err(io::Error::other)?;
        Ok((Some(bytes.len()), Box::new(io::Cursor::new(bytes))))
    }
}

/// Files from the project and packages, but the file being compiled from `text`.
struct Loader {
    files: SystemFiles,
    main: FileId,
    text: Mutex<Bytes>,
}

impl FileLoader for Loader {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.main {
            return Ok(self.text.lock().unwrap().clone());
        }
        if let VirtualRoot::Package(spec) = id.get().root() {
            have_package(spec).map_err(FileError::Package)?;
        }
        self.files.load(id)
    }
}

struct ProjectWorld {
    store: FileStore<Loader>,
    main: FileId,
    time: Time,
    /// Local time, as the editor said it is: the sandbox has no time zone.
    utc_offset_minutes: i32,
}

impl World for ProjectWorld {
    fn library(&self) -> &LazyHash<Library> {
        &env().library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        env().fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.store.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.store.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        env().fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.time.today(Some(offset.unwrap_or_else(|| Duration::from(time_offset(self.utc_offset_minutes)))))
    }
}

fn time_offset(minutes: i32) -> time::Duration {
    time::Duration::minutes(minutes.into())
}

/// One file, compiled again and again as it changes, and the pages it made.
struct Compiler {
    world: ProjectWorld,
    /// What's been made, by key: the last pages and the ones before, since those are
    /// on screen until the new ones have replaced them.
    pages: HashMap<u128, (u32, typst_layout::Page)>,
    made: u32,
}

impl Compiler {
    fn new(file: &str, utc_offset_minutes: i32) -> Result<Self, String> {
        let root = PathBuf::from("/project");
        let path = root.join(file);
        let virtual_path = VirtualPath::virtualize(&root, &path).map_err(|_| format!("{file} isn't in the project"))?;
        let main = RootedPath::new(VirtualRoot::Project, virtual_path).intern();
        let universe = UniversePackages::new(ByTheEditor);
        let packages = SystemPackages::from_parts(Some(FsPackages::new(YOUR_PACKAGES)), Some(FsPackages::new(DOWNLOADED_PACKAGES)), universe);
        let files = SystemFiles::new(FsRoot::new(root), packages);
        let loader = Loader { files, main, text: Mutex::new(Bytes::new(Vec::<u8>::new())) };
        let world = ProjectWorld { store: FileStore::new(loader), main, time: Time::system(), utc_offset_minutes };
        Ok(Self { world, pages: HashMap::new(), made: 0 })
    }

    fn compile(&mut self, text: &str) -> Result<Vec<Page>, String> {
        *self.world.store.loader().text.lock().unwrap() = Bytes::new(text.as_bytes().to_vec());
        self.world.store.reset();
        self.world.time.reset();
        let compiled = typst::compile::<PagedDocument>(&self.world).output;
        // Typst keeps what it computed, for next time: enough for that, not more.
        comemo::evict(10);
        let document = compiled.map_err(|errors| self.describe(&errors))?;
        self.made += 1;
        let mut pages = vec![];
        for page in document.pages() {
            let size = page.frame.size();
            let mut glyphs = vec![];
            collect(&page.frame, Transform::identity(), &self.world, &mut glyphs);
            let key = typst::utils::hash128(&page.frame);
            self.pages.insert(key, (self.made, page.clone()));
            pages.push(Page { width: size.x.to_pt() as f32, height: size.y.to_pt() as f32, key_high: (key >> 64) as u64, key_low: key as u64, glyphs });
        }
        let keep_from = self.made.saturating_sub(1);
        self.pages.retain(|_, (made, _)| *made >= keep_from);
        Ok(pages)
    }

    /// `path:line: error: message`, of the first error.
    fn describe(&self, errors: &[SourceDiagnostic]) -> String {
        let Some(error) = errors.first() else { return "typst failed".into() };
        let at = error.span.id().zip(self.world.range(error.span)).and_then(|(id, range)| {
            let line = self.world.source(id).ok()?.lines().byte_to_line(range.start)?;
            Some(format!("{}:{}: ", id.get().vpath().get_without_slash(), line + 1))
        });
        format!("{}error: {}", at.unwrap_or_default(), error.message)
    }

    fn draw(&self, key: u128, pixels: u32) -> Option<Drawing> {
        let (_, page) = self.pages.get(&key)?;
        let width = page.frame.size().x.to_pt() as f32;
        let height = page.frame.size().y.to_pt() as f32;
        let pixel_per_pt = pixels as f32 / width.max(1.);
        let across = |pt: f32| (pt * pixel_per_pt).ceil().max(1.) as usize;
        if across(width).saturating_mul(across(height)) > MAX_PIXELS {
            return None;
        }
        let options = typst_render::RenderOptions { pixel_per_pt: typst::utils::Scalar::new(pixel_per_pt.into()), ..Default::default() };
        let pixmap = typst_render::render(page, &options);
        let mut bgra = pixmap.data().to_vec();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Some(Drawing { width: pixmap.width(), height: pixmap.height(), bgra })
    }
}

/// The glyphs in `frame`, placed by `ts`, as they're laid out.
fn collect(frame: &Frame, ts: Transform, world: &ProjectWorld, out: &mut Vec<Glyph>) {
    for (pos, item) in frame.items() {
        let ts = ts.pre_concat(Transform::translate(pos.x, pos.y));
        match item {
            FrameItem::Group(group) => collect(&group.frame, ts.pre_concat(group.transform), world, out),
            FrameItem::Text(text) => {
                let metrics = text.font.metrics();
                let (top, bottom) = (-metrics.ascender.at(text.size), -metrics.descender.at(text.size));
                let mut x = Abs::zero();
                for glyph in &text.glyphs {
                    let left = x + glyph.x_offset.at(text.size);
                    let advance = glyph.x_advance.at(text.size);
                    x += advance;
                    let [a, b] = [Point::new(left, top), Point::new(left + advance, bottom)].map(|p| p.transform(ts));
                    let (span, offset) = glyph.span;
                    let source = (span.id() == Some(world.main)).then(|| world.range(span)).flatten().map(|range: Range<usize>| {
                        let from = (range.start + usize::from(offset)).min(range.end);
                        (from as u32, (from + glyph.range().len()).clamp(from, range.end.max(from + 1)) as u32)
                    });
                    out.push(Glyph {
                        x: a.x.min(b.x).to_pt() as f32,
                        y: a.y.min(b.y).to_pt() as f32,
                        width: (a.x - b.x).abs().to_pt() as f32,
                        height: (a.y - b.y).abs().to_pt() as f32,
                        source,
                    });
                }
            }
            _ => {}
        }
    }
}

/// The scope a tag shows as.
fn scope(tag: Tag) -> Option<&'static str> {
    Some(match tag {
        Tag::Comment => "comment",
        Tag::Punctuation => "punctuation",
        Tag::Escape => "string.escape",
        Tag::Strong => "text.strong",
        Tag::Emph => "text.emphasis",
        Tag::Link => "text.uri",
        Tag::Raw => "text.literal",
        Tag::Label | Tag::Ref => "text.reference",
        Tag::Heading => "text.title",
        Tag::ListMarker | Tag::ListTerm => "punctuation.special",
        Tag::MathDelimiter => "punctuation.delimiter",
        Tag::MathOperator | Tag::Operator => "operator",
        Tag::Keyword => "keyword",
        Tag::Number => "number",
        Tag::String => "string",
        Tag::Function => "function",
        Tag::Interpolated => "variable",
        _ => return None,
    })
}

/// The leaves under `node`, each with what it or what it's in is tagged as.
fn spans(node: &LinkedNode, inherited: Option<Tag>, out: &mut Vec<Span>) {
    let tag = tag_of(node).or(inherited);
    if node.children().len() == 0 {
        if let Some(scope) = tag.and_then(scope).filter(|_| !node.range().is_empty()) {
            let range = node.range();
            out.push(Span { start: range.start as u32, end: range.end as u32, scope: scope.to_string() });
        }
        return;
    }
    for child in node.children() {
        spans(&child, tag, out);
    }
}

thread_local! {
    static COMPILER: RefCell<Option<Compiler>> = const { RefCell::new(None) };
}

struct Typst;

impl Plugin for Typst {
    fn highlight(text: String) -> Vec<Span> {
        let tree = typst::syntax::parse(&text);
        let mut out = vec![];
        spans(&LinkedNode::new(&tree), None, &mut out);
        out
    }

    fn open_pages(path: String, utc_offset_minutes: i32) -> Result<(), String> {
        let compiler = Compiler::new(&path, utc_offset_minutes)?;
        COMPILER.with(|held| *held.borrow_mut() = Some(compiler));
        Ok(())
    }

    fn make_pages(text: String) -> Result<Vec<Page>, String> {
        COMPILER.with(|held| held.borrow_mut().as_mut().ok_or("no file is open".to_string())?.compile(&text))
    }

    fn draw_page(key_high: u64, key_low: u64, pixels: u32) -> Option<Drawing> {
        let key = (u128::from(key_high) << 64) | u128::from(key_low);
        COMPILER.with(|held| held.borrow().as_ref()?.draw(key, pixels))
    }
}

register!(Typst);
