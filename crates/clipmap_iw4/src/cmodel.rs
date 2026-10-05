use trace_iw4::{Trace, trace_capsule};

use crate::BrushView;

#[derive(Clone, Copy, Debug)]
pub struct ClipCmodel {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub radius: f32,
    pub first_brush: u32,
    pub num_brushes: u16,
}

pub fn clip_handle_to_model(cmodels: &[ClipCmodel], handle: u32) -> Option<&ClipCmodel> {
    cmodels.get(handle as usize)
}

pub fn transformed_capsule_trace<B: BrushView>(
    cmodel: &ClipCmodel,
    leafbrushes: &[u16],
    brushes: &[B],
    start: [f32; 3],
    end: [f32; 3],
    mins: [f32; 3],
    maxs: [f32; 3],
    origin: [f32; 3],
    angles: [f32; 3],
    mask: u32,
) -> Trace {
    if angles[0] == 0.0 && angles[1] == 0.0 && angles[2] == 0.0 {
        return transformed_capsule_trace_origin_only(
            cmodel,
            leafbrushes,
            brushes,
            start,
            end,
            mins,
            maxs,
            origin,
            mask,
        );
    }

    transformed_capsule_trace_rotated(
        cmodel,
        leafbrushes,
        brushes,
        start,
        end,
        mins,
        maxs,
        origin,
        angles,
        mask,
    )
}

fn transformed_capsule_trace_origin_only<B: BrushView>(
    cmodel: &ClipCmodel,
    leafbrushes: &[u16],
    brushes: &[B],
    start: [f32; 3],
    end: [f32; 3],
    mins: [f32; 3],
    maxs: [f32; 3],
    origin: [f32; 3],
    mask: u32,
) -> Trace {
    let local_start = [
        start[0] - origin[0],
        start[1] - origin[1],
        start[2] - origin[2],
    ];
    let local_end = [end[0] - origin[0], end[1] - origin[1], end[2] - origin[2]];
    let (lo, hi) = capsule_reach(mins, maxs);
    let mut hit = if misses_bounds(cmodel, local_start, local_end, lo, hi) {
        open_trace(local_start, local_end, mins, maxs, mask)
    } else {
        capsule_vs_cmodel(
            cmodel,
            leafbrushes,
            brushes,
            local_start,
            local_end,
            mins,
            maxs,
            mask,
        )
    };
    hit.endpos = [
        hit.endpos[0] + origin[0],
        hit.endpos[1] + origin[1],
        hit.endpos[2] + origin[2],
    ];
    hit
}

fn transformed_capsule_trace_rotated<B: BrushView>(
    cmodel: &ClipCmodel,
    leafbrushes: &[u16],
    brushes: &[B],
    start: [f32; 3],
    end: [f32; 3],
    mins: [f32; 3],
    maxs: [f32; 3],
    origin: [f32; 3],
    angles: [f32; 3],
    mask: u32,
) -> Trace {
    let matrix = angles_to_axis(angles);
    let mut start_l = [
        start[0] - origin[0],
        start[1] - origin[1],
        start[2] - origin[2],
    ];
    let mut end_l = [end[0] - origin[0], end[1] - origin[1], end[2] - origin[2]];
    rotate_point(&mut start_l, &matrix);
    rotate_point(&mut end_l, &matrix);

    // The box keeps its world axes while the segment turns into model space,
    // so its reach there is bounded by its farthest corner.
    let (lo, hi) = capsule_reach(mins, maxs);
    let reach = libm::sqrtf(
        (0..3)
            .map(|axis| {
                let far = libm::fabsf(lo[axis]).max(libm::fabsf(hi[axis]));
                far * far
            })
            .sum::<f32>(),
    );
    let mut hit = if misses_bounds(cmodel, start_l, end_l, [-reach; 3], [reach; 3]) {
        open_trace(start_l, end_l, mins, maxs, mask)
    } else {
        capsule_vs_cmodel(
            cmodel,
            leafbrushes,
            brushes,
            start_l,
            end_l,
            mins,
            maxs,
            mask,
        )
    };
    if hit.fraction < 1.0 {
        let transpose = transpose_matrix(&matrix);
        rotate_point(&mut hit.normal, &transpose);
    }
    let inv = transpose_matrix(&matrix);
    rotate_point(&mut hit.endpos, &inv);
    hit.endpos = [
        hit.endpos[0] + origin[0],
        hit.endpos[1] + origin[1],
        hit.endpos[2] + origin[2],
    ];
    hit
}

/// Slack around the model bounds for the reject test; brush traces clip
/// against surfaces a fraction of a unit out.
const BOUNDS_SLACK: f32 = 2.0;

/// Box bounds that hold the capsule `trace_capsule` sweeps for `mins`/`maxs`:
/// its radius follows the box's x half-size, so on y it can reach past a
/// narrow box.
fn capsule_reach(mins: [f32; 3], maxs: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mid = [
        (mins[0] + maxs[0]) * 0.5,
        (mins[1] + maxs[1]) * 0.5,
        (mins[2] + maxs[2]) * 0.5,
    ];
    let flat = libm::fabsf(maxs[0] - mid[0]).max(libm::fabsf(maxs[1] - mid[1]));
    let half = [flat, flat, libm::fabsf(maxs[2] - mid[2]).max(flat)];
    (
        [mid[0] - half[0], mid[1] - half[1], mid[2] - half[2]],
        [mid[0] + half[0], mid[1] + half[1], mid[2] + half[2]],
    )
}

/// The swept box (model space) cannot touch the model's bounds, so no brush
/// of it can be hit: the trace is open. Models with no usable bounds are
/// always traced.
fn misses_bounds(
    cmodel: &ClipCmodel,
    start: [f32; 3],
    end: [f32; 3],
    mins: [f32; 3],
    maxs: [f32; 3],
) -> bool {
    let usable = (0..3).all(|axis| {
        cmodel.mins[axis].is_finite()
            && cmodel.maxs[axis].is_finite()
            && cmodel.mins[axis] <= cmodel.maxs[axis]
    }) && cmodel.mins != cmodel.maxs;
    usable
        && (0..3).any(|axis| {
            start[axis].min(end[axis]) + mins[axis] - BOUNDS_SLACK > cmodel.maxs[axis]
                || start[axis].max(end[axis]) + maxs[axis] + BOUNDS_SLACK < cmodel.mins[axis]
        })
}

/// What `trace_capsule` returns when no brush is touched, bit for bit.
fn open_trace(start: [f32; 3], end: [f32; 3], mins: [f32; 3], maxs: [f32; 3], mask: u32) -> Trace {
    trace_capsule(core::iter::empty(), start, end, mins, maxs, mask)
}

fn capsule_vs_cmodel<B: BrushView>(
    cmodel: &ClipCmodel,
    leafbrushes: &[u16],
    brushes: &[B],
    start: [f32; 3],
    end: [f32; 3],
    mins: [f32; 3],
    maxs: [f32; 3],
    mask: u32,
) -> Trace {
    let first = cmodel.first_brush as usize;
    let last = first.saturating_add(cmodel.num_brushes as usize);
    let ids = leafbrushes.get(first..last).unwrap_or(&[]);
    let selected = ids
        .iter()
        .filter_map(|&id| brushes.get(id as usize))
        .map(crate::brush_ref);
    trace_capsule(selected, start, end, mins, maxs, mask)
}

fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    const DEG2RAD: f32 = 0.01745329238474369_f32;
    let yaw = angles[1] * DEG2RAD;
    let pitch = angles[0] * DEG2RAD;
    let roll = angles[2] * DEG2RAD;
    let cy = libm::cosf(yaw);
    let sy = libm::sinf(yaw);
    let cp = libm::cosf(pitch);
    let sp = libm::sinf(pitch);
    let cr = libm::cosf(roll);
    let sr = libm::sinf(roll);
    [
        [cp * cy, cp * sy, -sp],
        [sr * sp * cy + -sy * cr, sr * sp * sy + cr * cy, sr * cp],
        [cr * sp * cy + -sr * -sy, cr * sp * sy + -sr * cy, cr * cp],
    ]
}

fn rotate_point(point: &mut [f32; 3], mat: &[[f32; 3]; 3]) {
    let x = point[0];
    let y = point[1];
    let z = point[2];
    point[0] = mat[0][0] * x + mat[0][1] * y + mat[0][2] * z;
    point[1] = mat[1][0] * x + mat[1][1] * y + mat[1][2] * z;
    point[2] = mat[2][0] * x + mat[2][1] * y + mat[2][2] * z;
}

fn transpose_matrix(mat: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    [
        [mat[0][0], mat[1][0], mat[2][0]],
        [mat[0][1], mat[1][1], mat[2][1]],
        [mat[0][2], mat[1][2], mat[2][2]],
    ]
}
