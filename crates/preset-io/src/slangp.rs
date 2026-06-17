//! `.slangp` preset parser (Architecture §B; `docs/retroarch-slang-runtime.md`
//! §1 "Preset format & keys", §2 "Scale types").
//!
//! A `.slangp` is a flat INI-style `key = value` list with **no sections**.
//! Values may be quoted or unquoted; booleans accept `true`/`false`. All paths
//! are relative to the preset file's directory. `N` is the (zero-based) pass
//! index; passes run `0 .. shaders-1`.
//!
//! This module parses the documented keys into typed structs. Per the ticket it
//! is **forward-looking**: every documented per-pass key is captured as a typed
//! `Option` field now — even keys #22 does not yet consume (formats, samplers,
//! feedback, mipmap, LUTs, parameter overrides) so later tickets read parsed
//! fields rather than re-touching the parser. Defaults are *not* baked into the
//! `Option`s here; an unset key stays `None` so the engine can apply the
//! position-dependent defaults from §2 (intermediate = `source × 1.0`, final =
//! `viewport`). Convenience accessors ([`Pass::scale_type_x`] etc.) surface the
//! per-axis combined/override resolution where it is unambiguous.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Scale-type strings recognized by the RetroArch C parser (§2). The librashader
/// `Original` extension has no upstream preset string and is intentionally **not**
/// accepted (§11 open-question 1): an unknown string is a [`ParseError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleType {
    /// `source`: factor × this pass's input size (`OriginalSize` for pass 0,
    /// else the previous FBO size). RetroArch `RARCH_SCALE_INPUT`.
    Source,
    /// `viewport`: factor × the simulated final viewport size
    /// (`FinalViewportSize`). RetroArch `RARCH_SCALE_VIEWPORT`.
    Viewport,
    /// `absolute`: a literal integer pixel count; the input size is ignored.
    /// RetroArch `RARCH_SCALE_ABSOLUTE`.
    Absolute,
}

impl ScaleType {
    /// Parse a scale-type string. Only the three RetroArch C strings are valid.
    fn parse(s: &str) -> Result<Self, ParseError> {
        match s {
            "source" => Ok(ScaleType::Source),
            "viewport" => Ok(ScaleType::Viewport),
            "absolute" => Ok(ScaleType::Absolute),
            other => Err(ParseError::BadScaleType(other.to_string())),
        }
    }
}

/// Sampler wrap-mode strings (§3 `video_shader_wrap_str_to_mode`). An
/// unrecognized string maps to `ClampToBorder` (the RetroArch default), matching
/// upstream's lenient behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// `clamp_to_border` — RetroArch `RARCH_WRAP_BORDER`, the default.
    ClampToBorder,
    /// `clamp_to_edge` — RetroArch `RARCH_WRAP_EDGE`.
    ClampToEdge,
    /// `repeat` — RetroArch `RARCH_WRAP_REPEAT`.
    Repeat,
    /// `mirrored_repeat` — RetroArch `RARCH_WRAP_MIRRORED_REPEAT`.
    MirroredRepeat,
}

impl WrapMode {
    /// Parse a wrap-mode string; unrecognized → `ClampToBorder` (§3).
    fn parse(s: &str) -> Self {
        match s {
            "clamp_to_edge" => WrapMode::ClampToEdge,
            "repeat" => WrapMode::Repeat,
            "mirrored_repeat" => WrapMode::MirroredRepeat,
            // "clamp_to_border" and anything unrecognized:
            _ => WrapMode::ClampToBorder,
        }
    }
}

/// One pass of a `.slangp` preset (§1). Every documented per-pass key is a typed
/// `Option`; `None` means "key absent in the preset" so the engine can apply the
/// §2 position-dependent defaults. `shader` is the only required key and is
/// resolved to an absolute path against the preset directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    /// `shaderN` — the `.slang` file for this pass, resolved relative to the
    /// preset directory (always present; a pass without it is a [`ParseError`]).
    pub shader: PathBuf,
    /// `aliasN` — semantic name enabling `<alias>` / `<alias>Size` /
    /// `<alias>Feedback` bindings from later passes. (#24/reflection.)
    pub alias: Option<String>,

    // ---- Scale (§2; consumed by #22). ----
    /// `scale_typeN` — the combined scale type; `_x`/`_y` override per axis.
    pub scale_type: Option<ScaleType>,
    /// `scale_type_xN` — per-axis override of the combined scale type.
    pub scale_type_x: Option<ScaleType>,
    /// `scale_type_yN` — per-axis override of the combined scale type.
    pub scale_type_y: Option<ScaleType>,
    /// `scaleN` — the combined scale factor; if present `scale_x`/`scale_y` are
    /// ignored (§2). Stored raw as `f32`; `absolute` callers round to an int.
    pub scale: Option<f32>,
    /// `scale_xN` — per-axis scale factor (ignored if `scale` is present).
    pub scale_x: Option<f32>,
    /// `scale_yN` — per-axis scale factor (ignored if `scale` is present).
    pub scale_y: Option<f32>,

    // ---- Format / sampler / feedback (captured for #23/#24; not consumed yet). ----
    /// `filter_linearN` — `true`=linear, `false`=nearest filtering of the input.
    pub filter_linear: Option<bool>,
    /// `wrap_modeN` — sampler wrap mode for this pass's input.
    pub wrap_mode: Option<WrapMode>,
    /// `mipmap_inputN` — generate a mip chain for this pass's input texture.
    pub mipmap_input: Option<bool>,
    /// `float_framebufferN` — `true` → RGBA16F render target.
    pub float_framebuffer: Option<bool>,
    /// `srgb_framebufferN` — `true` → RGBA8 sRGB render target.
    pub srgb_framebuffer: Option<bool>,
    /// `frame_count_modN` — if `>0`, `FrameCount` fed to this pass wraps mod this.
    pub frame_count_mod: Option<u32>,
}

impl Pass {
    /// Effective X-axis scale type: the `_x` override if set, else the combined
    /// `scale_type`. `None` ⇒ no scale keys for this axis (engine applies the §2
    /// position default). Does not invent a default.
    pub fn scale_type_x(&self) -> Option<ScaleType> {
        self.scale_type_x.or(self.scale_type)
    }

    /// Effective Y-axis scale type (see [`Pass::scale_type_x`]).
    pub fn scale_type_y(&self) -> Option<ScaleType> {
        self.scale_type_y.or(self.scale_type)
    }

    /// Effective X-axis scale factor: the combined `scale` wins over `scale_x`
    /// (§2: "if `scaleN` is present, `scale_xN`/`scale_yN` are ignored").
    pub fn scale_factor_x(&self) -> Option<f32> {
        self.scale.or(self.scale_x)
    }

    /// Effective Y-axis scale factor (see [`Pass::scale_factor_x`]).
    pub fn scale_factor_y(&self) -> Option<f32> {
        self.scale.or(self.scale_y)
    }

    /// Whether the pass declares any scale key at all. A pass with no scale keys
    /// is not `FBO_SCALE_FLAG_VALID` (§2) and takes the position-dependent
    /// default (intermediate `source × 1.0`, final `viewport`).
    pub fn has_scale(&self) -> bool {
        self.scale_type.is_some()
            || self.scale_type_x.is_some()
            || self.scale_type_y.is_some()
            || self.scale.is_some()
            || self.scale_x.is_some()
            || self.scale_y.is_some()
    }
}

/// A LUT (`textures` family, §1; consumed by #27). Captured but not yet used.
#[derive(Debug, Clone, PartialEq)]
pub struct LutEntry {
    /// The LUT name as listed in `textures = "A;B"` and bound as `<NAME>`.
    pub name: String,
    /// `<NAME>` — the image path, resolved relative to the preset directory.
    pub path: PathBuf,
    /// `<NAME>_linear` — `true`=linear, `false`=nearest (LUTs default nearest).
    pub linear: Option<bool>,
    /// `<NAME>_wrap_mode` — sampler wrap for the LUT.
    pub wrap_mode: Option<WrapMode>,
    /// `<NAME>_mipmap` — generate mips for the LUT.
    pub mipmap: Option<bool>,
}

/// A parsed `.slangp` preset (§1). Per-pass data is in `passes`; preset-level
/// keys (`feedback_pass`, LUTs, parameter overrides) live alongside.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// The preset's directory — the base for resolving every relative path.
    pub base_dir: PathBuf,
    /// The passes in chain order (`passes.len() == shaders`).
    pub passes: Vec<Pass>,
    /// `feedback_pass` — global pass index double-buffered for feedback; `None`
    /// when absent (RetroArch default `-1` = no global feedback pass). (#24.)
    pub feedback_pass: Option<i32>,
    /// `textures` LUT family (#27): one [`LutEntry`] per name in `textures`.
    pub luts: Vec<LutEntry>,
    /// Bare `id = value` parameter overrides (§8). Captured but not consumed yet;
    /// these override a `#pragma parameter` initial value at runtime.
    pub parameter_overrides: BTreeMap<String, f32>,
    /// Every key the parser did **not** recognize as a structural key *and* could
    /// not interpret as a float parameter override — retained verbatim so import
    /// never silently drops data (the importer surfaces these as diagnostics).
    /// This is distinct from [`Preset::parameter_overrides`]: a key lands in
    /// exactly one of the two (structural keys go into typed fields instead).
    pub extras: BTreeMap<String, String>,
}

/// Upper bound on the declared `shaders` pass count we will accept (B1). The
/// `shaders` value comes from an untrusted preset file fed to a registered Tauri
/// command (`load_preset`), so it must be validated before it is used to size a
/// `Vec`: an absurd count (e.g. `shaders = 8800000000000`) would otherwise make
/// `Vec::with_capacity` request terabytes and `SIGABRT` the whole process — an
/// allocation abort that `catch_unwind` does NOT contain. RetroArch's
/// `GFX_MAX_SHADERS` is 26; we use a generous-but-sane cap so any real preset
/// passes while a hostile/garbage count fails cleanly with a [`ParseError`].
const MAX_PASSES: usize = 64;

/// Errors parsing a `.slangp` preset.
#[derive(Debug)]
pub enum ParseError {
    /// The preset file could not be read.
    Io(std::io::Error),
    /// A required key (`shaders`, or a `shaderN` for some `N`) is missing.
    MissingKey(String),
    /// A value could not be parsed as the expected type.
    BadValue { key: String, value: String },
    /// A `scale_type*` value was not one of `source`/`viewport`/`absolute`.
    BadScaleType(String),
    /// A LUT named in `textures` has no `<NAME> = path` entry.
    MissingLut(String),
    /// The declared `shaders` count exceeds [`MAX_PASSES`] — refused rather than
    /// trusted, so an untrusted preset cannot drive an unbounded allocation (B1).
    TooManyPasses { declared: usize, max: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "could not read preset: {e}"),
            ParseError::MissingKey(k) => write!(f, "missing required preset key `{k}`"),
            ParseError::BadValue { key, value } => {
                write!(f, "invalid value `{value}` for preset key `{key}`")
            }
            ParseError::BadScaleType(s) => write!(
                f,
                "unknown scale type `{s}` (expected source/viewport/absolute)"
            ),
            ParseError::MissingLut(n) => write!(f, "LUT `{n}` listed in `textures` has no path"),
            ParseError::TooManyPasses { declared, max } => write!(
                f,
                "preset declares {declared} passes, which exceeds the maximum of {max}"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

/// Read and parse a `.slangp` file from disk. Relative `shaderN`/LUT paths are
/// resolved against the file's directory.
pub fn parse_slangp(path: impl AsRef<Path>) -> Result<Preset, ParseError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(ParseError::Io)?;
    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    parse_slangp_str(&text, &base_dir)
}

/// Parse `.slangp` text already in memory, resolving relative paths against
/// `base_dir`. Split out from [`parse_slangp`] so it can be unit-tested without
/// touching the filesystem.
pub fn parse_slangp_str(text: &str, base_dir: &Path) -> Result<Preset, ParseError> {
    let map = parse_ini(text);

    let shaders: usize = require_parse(&map, "shaders")?;

    // VALIDATE the untrusted count BEFORE using it to size anything (B1). A
    // hostile preset (`shaders = 8800000000000`) would otherwise make
    // `Vec::with_capacity` request terabytes and `SIGABRT` the whole process —
    // an allocation abort `catch_unwind` cannot contain. Refuse anything above
    // the sane cap; `Vec::new()` then grows as real `shaderN` keys are found.
    if shaders > MAX_PASSES {
        return Err(ParseError::TooManyPasses {
            declared: shaders,
            max: MAX_PASSES,
        });
    }

    let mut passes = Vec::new();
    for n in 0..shaders {
        passes.push(parse_pass(&map, n, base_dir)?);
    }

    let feedback_pass = opt_parse::<i32>(&map, "feedback_pass")?;
    let luts = parse_luts(&map, base_dir)?;
    let (parameter_overrides, extras) = parse_overrides_and_extras(&map, &passes, &luts);

    Ok(Preset {
        base_dir: base_dir.to_path_buf(),
        passes,
        feedback_pass,
        luts,
        parameter_overrides,
        extras,
    })
}

/// Parse the flat INI body into a `key -> value` map. Whole-line and **inline**
/// comments (`#`/`//`) and blank lines are skipped; surrounding whitespace and a
/// single pair of quotes are stripped from each value. Later duplicate keys win
/// (RetroArch behavior).
fn parse_ini(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        // Strip a trailing inline comment before unquoting: many real presets
        // (e.g. crt-royale) write `scale_y5 = "0.0625" # note` or
        // `filter_linear7 = "true" // note`. Only a comment marker OUTSIDE quotes
        // counts, so a quoted value containing `#`/`//` (e.g. a path) is preserved.
        map.insert(key, unquote(strip_inline_comment(value).trim()).to_string());
    }
    map
}

/// Truncate `value` at the first `#` or `//` comment marker that is NOT inside a
/// double-quoted span. A value with no such marker is returned unchanged.
fn strip_inline_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b'#' if !in_quotes => return &value[..i],
            b'/' if !in_quotes && bytes.get(i + 1) == Some(&b'/') => return &value[..i],
            _ => {}
        }
        i += 1;
    }
    value
}

/// Strip one pair of surrounding double quotes from a value, if present.
fn unquote(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Parse a single pass's keys (`shaderN`, scale, format/sampler/feedback) into a
/// [`Pass`]. `shaderN` is required; everything else stays `None` if absent.
fn parse_pass(
    map: &BTreeMap<String, String>,
    n: usize,
    base_dir: &Path,
) -> Result<Pass, ParseError> {
    let shader_key = format!("shader{n}");
    let shader_rel = map
        .get(&shader_key)
        .ok_or_else(|| ParseError::MissingKey(shader_key.clone()))?;
    let shader = resolve(base_dir, shader_rel);

    Ok(Pass {
        shader,
        alias: map.get(&format!("alias{n}")).cloned(),

        scale_type: opt_scale_type(map, &format!("scale_type{n}"))?,
        scale_type_x: opt_scale_type(map, &format!("scale_type_x{n}"))?,
        scale_type_y: opt_scale_type(map, &format!("scale_type_y{n}"))?,
        scale: opt_parse::<f32>(map, &format!("scale{n}"))?,
        scale_x: opt_parse::<f32>(map, &format!("scale_x{n}"))?,
        scale_y: opt_parse::<f32>(map, &format!("scale_y{n}"))?,

        filter_linear: opt_bool(map, &format!("filter_linear{n}"))?,
        wrap_mode: map
            .get(&format!("wrap_mode{n}"))
            .map(|s| WrapMode::parse(s)),
        mipmap_input: opt_bool(map, &format!("mipmap_input{n}"))?,
        float_framebuffer: opt_bool(map, &format!("float_framebuffer{n}"))?,
        srgb_framebuffer: opt_bool(map, &format!("srgb_framebuffer{n}"))?,
        frame_count_mod: opt_parse::<u32>(map, &format!("frame_count_mod{n}"))?,
    })
}

/// Parse the `textures = "A;B"` LUT family into [`LutEntry`]s. Each listed name
/// needs a `<NAME> = path` entry; its `_linear`/`_wrap_mode`/`_mipmap` keys are
/// captured if present.
fn parse_luts(
    map: &BTreeMap<String, String>,
    base_dir: &Path,
) -> Result<Vec<LutEntry>, ParseError> {
    let Some(list) = map.get("textures") else {
        return Ok(Vec::new());
    };
    let mut luts = Vec::new();
    for name in list.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let path_rel = map
            .get(name)
            .ok_or_else(|| ParseError::MissingLut(name.to_string()))?;
        luts.push(LutEntry {
            name: name.to_string(),
            path: resolve(base_dir, path_rel),
            linear: opt_bool(map, &format!("{name}_linear"))?,
            wrap_mode: map
                .get(&format!("{name}_wrap_mode"))
                .map(|s| WrapMode::parse(s)),
            mipmap: opt_bool(map, &format!("{name}_mipmap"))?,
        });
    }
    Ok(luts)
}

/// Partition every non-structural key into parameter overrides vs. preserved
/// extras, so **no key in the preset is silently lost** (the importer surfaces
/// the extras as diagnostics).
///
/// A key that is not a recognized structural key (those become typed fields) is:
/// - a **parameter override** (§8) if its value parses as a float — these
///   override a `#pragma parameter` initial value at runtime; or
/// - an **extra** otherwise — retained verbatim in [`Preset::extras`].
///
/// The informational `parameters = "..."` list is *not* required for either.
fn parse_overrides_and_extras(
    map: &BTreeMap<String, String>,
    passes: &[Pass],
    luts: &[LutEntry],
) -> (BTreeMap<String, f32>, BTreeMap<String, String>) {
    let mut overrides = BTreeMap::new();
    let mut extras = BTreeMap::new();
    for (key, value) in map {
        if is_structural_key(key, passes.len(), luts) {
            continue;
        }
        match value.parse::<f32>() {
            Ok(v) => {
                overrides.insert(key.clone(), v);
            }
            Err(_) => {
                extras.insert(key.clone(), value.clone());
            }
        }
    }
    (overrides, extras)
}

/// Whether `key` is a structural preset key (vs. a bare parameter override).
/// Structural keys: the globals, every documented per-pass key for any `N`, the
/// `textures` list itself, and any LUT name + its `_*` sub-keys.
fn is_structural_key(key: &str, pass_count: usize, luts: &[LutEntry]) -> bool {
    const GLOBALS: &[&str] = &["shaders", "feedback_pass", "textures", "parameters"];
    if GLOBALS.contains(&key) {
        return true;
    }
    // Per-pass keys: `<prefix><N>`.
    const PASS_PREFIXES: &[&str] = &[
        "shader",
        "alias",
        "scale_type_x",
        "scale_type_y",
        "scale_type",
        "scale_x",
        "scale_y",
        "scale",
        "filter_linear",
        "wrap_mode",
        "mipmap_input",
        "float_framebuffer",
        "srgb_framebuffer",
        "frame_count_mod",
    ];
    for prefix in PASS_PREFIXES {
        if let Some(rest) = key.strip_prefix(prefix) {
            if let Ok(n) = rest.parse::<usize>() {
                if n < pass_count {
                    return true;
                }
            }
        }
    }
    // LUT names and their sub-keys.
    for lut in luts {
        if key == lut.name
            || key == format!("{}_linear", lut.name)
            || key == format!("{}_wrap_mode", lut.name)
            || key == format!("{}_mipmap", lut.name)
        {
            return true;
        }
    }
    false
}

/// Resolve a (possibly relative) path against the preset directory. Absolute
/// paths are returned unchanged.
fn resolve(base_dir: &Path, rel: &str) -> PathBuf {
    let p = Path::new(rel);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

/// Look up a required key and parse it, erroring if absent or unparseable.
fn require_parse<T: std::str::FromStr>(
    map: &BTreeMap<String, String>,
    key: &str,
) -> Result<T, ParseError> {
    let value = map
        .get(key)
        .ok_or_else(|| ParseError::MissingKey(key.to_string()))?;
    value.parse::<T>().map_err(|_| ParseError::BadValue {
        key: key.to_string(),
        value: value.clone(),
    })
}

/// Parse an optional typed key; `Ok(None)` if absent, error if present-but-bad.
fn opt_parse<T: std::str::FromStr>(
    map: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<T>, ParseError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => value
            .parse::<T>()
            .map(Some)
            .map_err(|_| ParseError::BadValue {
                key: key.to_string(),
                value: value.clone(),
            }),
    }
}

/// Parse an optional boolean key (`true`/`false`, case-insensitive; also `1`/`0`).
fn opt_bool(map: &BTreeMap<String, String>, key: &str) -> Result<Option<bool>, ParseError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Some(true)),
            "false" | "0" => Ok(Some(false)),
            _ => Err(ParseError::BadValue {
                key: key.to_string(),
                value: value.clone(),
            }),
        },
    }
}

/// Parse an optional scale-type key, validating against the three C strings.
fn opt_scale_type(
    map: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<ScaleType>, ParseError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => ScaleType::parse(value).map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small but representative two-pass preset exercising the documented key
    // set: counts, paths, combined + per-axis scale, format/sampler/feedback
    // hints, a LUT family, and a bare parameter override.
    const FIXTURE: &str = r#"
# A comment line
shaders = 2
feedback_pass = 1

shader0 = "shaders/first.slang"
alias0 = FirstPass
scale_type0 = source
scale0 = 2.0
filter_linear0 = true
srgb_framebuffer0 = true

shader1 = shaders/second.slang
scale_type_x1 = absolute
scale_x1 = 320
scale_type_y1 = viewport
scale_y1 = 1.0
float_framebuffer1 = true
wrap_mode1 = repeat
mipmap_input1 = true
frame_count_mod1 = 60

textures = "BORDER;OVERLAY"
BORDER = luts/border.png
BORDER_linear = true
BORDER_wrap_mode = clamp_to_edge
OVERLAY = luts/overlay.png

parameters = "BRIGHT;CONTRAST"
BRIGHT = 1.5
CONTRAST = 0.75
"#;

    fn parse_fixture() -> Preset {
        parse_slangp_str(FIXTURE, Path::new("/presets")).expect("fixture parses")
    }

    #[test]
    fn parses_pass_count_and_resolved_paths() {
        let p = parse_fixture();
        assert_eq!(p.passes.len(), 2, "two passes");
        // shaderN paths are resolved relative to the preset dir.
        assert_eq!(
            p.passes[0].shader,
            PathBuf::from("/presets/shaders/first.slang")
        );
        assert_eq!(
            p.passes[1].shader,
            PathBuf::from("/presets/shaders/second.slang")
        );
        assert_eq!(p.base_dir, PathBuf::from("/presets"));
    }

    #[test]
    fn parses_combined_scale_keys() {
        let p = parse_fixture();
        let pass0 = &p.passes[0];
        assert_eq!(pass0.alias.as_deref(), Some("FirstPass"));
        assert_eq!(pass0.scale_type, Some(ScaleType::Source));
        assert_eq!(pass0.scale, Some(2.0));
        // Combined scale applies to both axes.
        assert_eq!(pass0.scale_type_x(), Some(ScaleType::Source));
        assert_eq!(pass0.scale_type_y(), Some(ScaleType::Source));
        assert_eq!(pass0.scale_factor_x(), Some(2.0));
        assert_eq!(pass0.scale_factor_y(), Some(2.0));
        assert!(pass0.has_scale());
    }

    #[test]
    fn parses_per_axis_scale_overrides() {
        let p = parse_fixture();
        let pass1 = &p.passes[1];
        assert_eq!(pass1.scale_type, None, "no combined type on pass 1");
        assert_eq!(pass1.scale_type_x(), Some(ScaleType::Absolute));
        assert_eq!(pass1.scale_type_y(), Some(ScaleType::Viewport));
        assert_eq!(pass1.scale_factor_x(), Some(320.0));
        assert_eq!(pass1.scale_factor_y(), Some(1.0));
    }

    #[test]
    fn captures_format_sampler_feedback_hints() {
        let p = parse_fixture();
        assert_eq!(p.passes[0].filter_linear, Some(true));
        assert_eq!(p.passes[0].srgb_framebuffer, Some(true));
        assert_eq!(p.passes[0].float_framebuffer, None);
        assert_eq!(p.passes[1].float_framebuffer, Some(true));
        assert_eq!(p.passes[1].wrap_mode, Some(WrapMode::Repeat));
        assert_eq!(p.passes[1].mipmap_input, Some(true));
        assert_eq!(p.passes[1].frame_count_mod, Some(60));
        assert_eq!(p.feedback_pass, Some(1));
    }

    #[test]
    fn captures_lut_family() {
        let p = parse_fixture();
        assert_eq!(p.luts.len(), 2);
        let border = &p.luts[0];
        assert_eq!(border.name, "BORDER");
        assert_eq!(border.path, PathBuf::from("/presets/luts/border.png"));
        assert_eq!(border.linear, Some(true));
        assert_eq!(border.wrap_mode, Some(WrapMode::ClampToEdge));
        assert_eq!(border.mipmap, None);
        assert_eq!(p.luts[1].name, "OVERLAY");
    }

    #[test]
    fn captures_parameter_overrides() {
        let p = parse_fixture();
        assert_eq!(p.parameter_overrides.get("BRIGHT"), Some(&1.5));
        assert_eq!(p.parameter_overrides.get("CONTRAST"), Some(&0.75));
        // Structural keys never leak into the overrides map.
        assert!(!p.parameter_overrides.contains_key("scale0"));
        assert!(!p.parameter_overrides.contains_key("shaders"));
        assert!(!p.parameter_overrides.contains_key("BORDER"));
        assert!(!p.parameter_overrides.contains_key("frame_count_mod1"));
        // The fixture has no unknown keys -> nothing preserved as extras.
        assert!(p.extras.is_empty(), "fixture has no unrecognized keys");
    }

    #[test]
    fn unrecognized_keys_preserved_as_extras() {
        // Non-structural, non-float keys are kept verbatim in `extras` (not
        // dropped, and not mistaken for parameter overrides).
        let p = parse_slangp_str(
            "shaders = 1\n\
             shader0 = a.slang\n\
             some_future_key = some_string_value\n\
             vendor_flag = on\n\
             GAMMA = 2.2\n",
            Path::new("/p"),
        )
        .expect("preset with unknown keys parses");
        // Float-valued unknowns are parameter overrides; non-float unknowns are extras.
        assert_eq!(p.parameter_overrides.get("GAMMA"), Some(&2.2));
        assert_eq!(
            p.extras.get("some_future_key").map(String::as_str),
            Some("some_string_value")
        );
        assert_eq!(p.extras.get("vendor_flag").map(String::as_str), Some("on"));
        // A key lands in exactly one bucket.
        assert!(!p.extras.contains_key("GAMMA"));
        assert!(!p.parameter_overrides.contains_key("some_future_key"));
    }

    #[test]
    fn no_key_is_silently_lost() {
        // The contract: every key in the raw INI body is accounted for — it is
        // either structural (consumed into a typed field), a float parameter
        // override, or a preserved extra. Nothing is dropped.
        let text = "shaders = 1\n\
             shader0 = a.slang\n\
             scale_type0 = source\n\
             scale0 = 2.0\n\
             alias0 = MAIN\n\
             filter_linear0 = true\n\
             feedback_pass = 0\n\
             textures = LUT\n\
             LUT = lut.png\n\
             LUT_linear = true\n\
             parameters = \"P;Q\"\n\
             P = 1.0\n\
             Q = 0.5\n\
             custom_unknown = hello world\n\
             another_unknown = 42\n";
        let raw = parse_ini(text);
        let p = parse_slangp_str(text, Path::new("/p")).expect("parses");

        for key in raw.keys() {
            let structural = is_structural_key(key, p.passes.len(), &p.luts);
            let is_override = p.parameter_overrides.contains_key(key);
            let is_extra = p.extras.contains_key(key);
            assert!(
                structural || is_override || is_extra,
                "key `{key}` was silently lost (not structural, override, or extra)"
            );
            // And it lands in exactly one non-structural bucket.
            assert!(
                !(is_override && is_extra),
                "key `{key}` is in both overrides and extras"
            );
        }
        // Sanity: the unknowns ended up where expected.
        assert_eq!(
            p.extras.get("custom_unknown").map(String::as_str),
            Some("hello world")
        );
        assert_eq!(p.parameter_overrides.get("another_unknown"), Some(&42.0));
    }

    #[test]
    fn strips_inline_comments_after_values() {
        // Real presets (crt-royale) write inline `#`/`//` comments after a value,
        // including after a quoted bool/number. Both must be stripped, and a `#`
        // INSIDE quotes (e.g. a path) must be preserved.
        assert_eq!(
            strip_inline_comment("\"0.0625\" # Safe for >= 341 triads"),
            "\"0.0625\" "
        );
        assert_eq!(
            strip_inline_comment("\"true\" // could be nearest"),
            "\"true\" "
        );
        assert_eq!(strip_inline_comment("2.4"), "2.4");
        assert_eq!(
            strip_inline_comment("\"shaders/a#b.slang\""),
            "\"shaders/a#b.slang\""
        );
        assert_eq!(
            strip_inline_comment("\"path//x.slang\""),
            "\"path//x.slang\""
        );

        // End-to-end: a preset whose pass keys carry inline comments parses the
        // clean values (the crt-royale failure mode).
        let p = parse_slangp_str(
            "shaders = 1 # one pass\n\
             shader0 = a.slang\n\
             filter_linear0 = \"true\" # could be nearest\n\
             scale_x0 = \"2.0\"  // double width\n\
             scale_type_x0 = source\n",
            Path::new("/p"),
        )
        .expect("inline-comment preset parses");
        assert_eq!(p.passes.len(), 1, "`shaders = 1 # ...` parses to 1");
        assert_eq!(p.passes[0].filter_linear, Some(true), "trailing # stripped");
        assert_eq!(
            p.passes[0].scale_factor_x(),
            Some(2.0),
            "trailing // stripped"
        );
    }

    #[test]
    fn defaults_are_none_when_keys_absent() {
        // A minimal one-pass preset: only `shaders` + `shader0`.
        let p = parse_slangp_str("shaders = 1\nshader0 = a.slang\n", Path::new("/p"))
            .expect("minimal preset parses");
        assert_eq!(p.passes.len(), 1);
        let pass = &p.passes[0];
        assert_eq!(pass.shader, PathBuf::from("/p/a.slang"));
        assert!(
            !pass.has_scale(),
            "no scale keys -> position default applies"
        );
        assert_eq!(pass.scale_type_x(), None);
        assert_eq!(pass.scale_factor_x(), None);
        assert_eq!(pass.filter_linear, None);
        assert_eq!(pass.float_framebuffer, None);
        assert_eq!(p.feedback_pass, None);
        assert!(p.luts.is_empty());
        assert!(p.parameter_overrides.is_empty());
    }

    #[test]
    fn missing_shaders_key_errors() {
        let err = parse_slangp_str("shader0 = a.slang\n", Path::new("/p")).unwrap_err();
        assert!(matches!(err, ParseError::MissingKey(k) if k == "shaders"));
    }

    #[test]
    fn missing_shader_for_declared_pass_errors() {
        let err =
            parse_slangp_str("shaders = 2\nshader0 = a.slang\n", Path::new("/p")).unwrap_err();
        assert!(matches!(err, ParseError::MissingKey(k) if k == "shader1"));
    }

    #[test]
    fn absurd_shader_count_errors_cleanly_without_aborting() {
        // B1: a hostile/garbage `shaders` count must NOT be trusted to pre-size a
        // Vec (which would request terabytes and SIGABRT the whole process — an
        // allocation abort `catch_unwind` does not contain). It must return a
        // clean Err instead, so `load_preset` (a Tauri command on a frontend path)
        // cannot take down the app.
        let err = parse_slangp_str("shaders = 8800000000000\n", Path::new("/p")).unwrap_err();
        assert!(
            matches!(err, ParseError::TooManyPasses { declared, max }
                if declared == 8_800_000_000_000 && max == MAX_PASSES),
            "huge shader count must error as TooManyPasses, got {err:?}"
        );

        // A count just over the cap is also refused (boundary), and the cap itself
        // is accepted up to the point a `shaderN` key is actually missing.
        let err = parse_slangp_str(&format!("shaders = {}\n", MAX_PASSES + 1), Path::new("/p"))
            .unwrap_err();
        assert!(matches!(err, ParseError::TooManyPasses { .. }));
        // At the cap, the count is allowed through to normal per-pass parsing
        // (which then fails on the first missing `shaderN`, NOT on allocation).
        let err =
            parse_slangp_str(&format!("shaders = {MAX_PASSES}\n"), Path::new("/p")).unwrap_err();
        assert!(matches!(err, ParseError::MissingKey(k) if k == "shader0"));
    }

    #[test]
    fn unknown_scale_type_errors() {
        let err = parse_slangp_str(
            "shaders = 1\nshader0 = a.slang\nscale_type0 = original\n",
            Path::new("/p"),
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::BadScaleType(s) if s == "original"));
    }
}
