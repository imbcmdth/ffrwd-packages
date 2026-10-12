//! The passes, in WGSL. Each is a module of its own, its bindings numbered
//! in the order [`crate::gpu`] lists their slots, and each starts with
//! [`COMMON`].
//!
//! Everything is counted in 8-bit code values: a texel of an `rgba8unorm`
//! texture is read back to the byte it holds, the filtered picture is
//! rounded to bytes before it is composited, and the composite is integer
//! arithmetic, so a matte of 255 gives the filtered bytes and a matte of 0
//! the base bytes exactly.

/// The parameters every pass reads, and the composite.
pub const COMMON: &str = r#"
struct Params {
    width: u32,
    height: u32,
    radius: u32,
    size: u32,
    blocks_x: u32,
    blocks_y: u32,
    pad0: u32,
    pad1: u32,
}

// The bytes an rgba8unorm texel holds.
fn bytes(texel: vec4<f32>) -> vec4<f32> {
    return round(texel * 255.0);
}

// `base + (filtered - base) * m / 255`, rounded to the nearest byte, as
// `(base * (255 - m) + filtered * m + 127) / 255`; opaque.
fn composite(base: vec4<u32>, filtered: vec4<u32>, m: u32) -> vec4<f32> {
    let c = (base.rgb * (255u - m) + filtered.rgb * m + vec3<u32>(127u)) / 255u;
    return vec4<f32>(vec3<f32>(c) / 255.0, 1.0);
}
"#;

/// The blur along each row: `v`'s bytes weighed by the kernel, edges
/// clamped, kept as floats.
pub const BLUR_ROWS: &str = r#"
@group(0) @binding(0) var v: texture_2d<f32>;
@group(0) @binding(1) var<uniform> p: Params;
@group(0) @binding(2) var<storage, read> weights: array<f32>;
@group(0) @binding(3) var rows: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.width || id.y >= p.height) {
        return;
    }
    let r = i32(p.radius);
    let last = i32(p.width) - 1;
    let y = i32(id.y);
    var sum = vec4<f32>(0.0);
    for (var k = -r; k <= r; k++) {
        let x = clamp(i32(id.x) + k, 0, last);
        sum += weights[abs(k)] * bytes(textureLoad(v, vec2<i32>(x, y), 0));
    }
    textureStore(rows, vec2<i32>(id.xy), sum);
}
"#;

/// The blur down each column of the rows pass, rounded to bytes, and the
/// composite. Where the matte is 0 the column is not summed: the base is
/// the answer whatever it would have been.
pub const BLUR_COLUMNS: &str = r#"
@group(0) @binding(0) var v: texture_2d<f32>;
@group(0) @binding(1) var matte: texture_2d<f32>;
@group(0) @binding(2) var<uniform> p: Params;
@group(0) @binding(3) var<storage, read> weights: array<f32>;
@group(0) @binding(4) var rows: texture_2d<f32>;
@group(0) @binding(5) var dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.width || id.y >= p.height) {
        return;
    }
    let at = vec2<i32>(id.xy);
    let base = vec4<u32>(bytes(textureLoad(v, at, 0)));
    let m = u32(bytes(textureLoad(matte, at, 0)).r);
    var filtered = base;
    if (m != 0u) {
        let r = i32(p.radius);
        let last = i32(p.height) - 1;
        var sum = vec4<f32>(0.0);
        for (var k = -r; k <= r; k++) {
            let y = clamp(at.y + k, 0, last);
            sum += weights[abs(k)] * textureLoad(rows, vec2<i32>(at.x, y), 0);
        }
        filtered = vec4<u32>(clamp(floor(sum + 0.5), vec4<f32>(0.0), vec4<f32>(255.0)));
    }
    textureStore(dst, at, composite(base, filtered, m));
}
"#;

/// The mosaic's first half: the sum of `v`'s bytes along each row of each
/// block, one invocation a block column and a row. A block at the right
/// edge is as wide as what is left of the frame.
pub const MOSAIC_ROWS: &str = r#"
@group(0) @binding(0) var v: texture_2d<f32>;
@group(0) @binding(1) var<uniform> p: Params;
@group(0) @binding(2) var<storage, read_write> sums: array<vec4<u32>>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.blocks_x || id.y >= p.height) {
        return;
    }
    let x0 = id.x * p.size;
    let x1 = min(x0 + p.size, p.width);
    var sum = vec4<u32>(0u);
    for (var x = x0; x < x1; x++) {
        sum += vec4<u32>(bytes(textureLoad(v, vec2<i32>(i32(x), i32(id.y)), 0)));
    }
    sums[id.y * p.blocks_x + id.x] = sum;
}
"#;

/// The mosaic's second half: each block's rows added up and divided by its
/// pixel count, truncating, as ffmpeg's `pixelize` does. A block at the
/// bottom or right edge counts only the pixels it has.
pub const MOSAIC_BLOCKS: &str = r#"
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> sums: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read_write> blocks: array<vec4<u32>>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.blocks_x || id.y >= p.blocks_y) {
        return;
    }
    let y0 = id.y * p.size;
    let y1 = min(y0 + p.size, p.height);
    let w = min(p.size, p.width - id.x * p.size);
    var sum = vec4<u32>(0u);
    for (var y = y0; y < y1; y++) {
        sum += sums[y * p.blocks_x + id.x];
    }
    blocks[id.y * p.blocks_x + id.x] = sum / (w * (y1 - y0));
}
"#;

/// Each pixel's block, and the composite.
pub const MOSAIC_COMPOSITE: &str = r#"
@group(0) @binding(0) var v: texture_2d<f32>;
@group(0) @binding(1) var matte: texture_2d<f32>;
@group(0) @binding(2) var<uniform> p: Params;
@group(0) @binding(3) var<storage, read> blocks: array<vec4<u32>>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.width || id.y >= p.height) {
        return;
    }
    let at = vec2<i32>(id.xy);
    let base = vec4<u32>(bytes(textureLoad(v, at, 0)));
    let m = u32(bytes(textureLoad(matte, at, 0)).r);
    var filtered = base;
    if (m != 0u) {
        filtered = blocks[(id.y / p.size) * p.blocks_x + id.x / p.size];
    }
    textureStore(dst, at, composite(base, filtered, m));
}
"#;

/// One pass's source: [`COMMON`] and the pass.
pub fn source(pass: &str) -> String {
    format!("{COMMON}{pass}")
}
