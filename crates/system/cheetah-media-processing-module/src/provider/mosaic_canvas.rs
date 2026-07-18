//! YUV420p canvas allocation and tile compositing helpers for the video mosaic.

use cheetah_media_api::processing::MosaicCell;

use avcodec::core::{BufferHandle, FitMode, Image, ImageInfo, ImagePlane, Rect, SampleLayout};

const STRIDE_ALIGNMENT: usize = 64;

fn align_up(n: usize, a: usize) -> usize {
    n.div_ceil(a) * a
}

pub(crate) fn allocate_canvas_buffer(width: u32, height: u32) -> (Vec<u8>, usize, usize) {
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let y_stride = align_up(w, STRIDE_ALIGNMENT);
    let uv_stride = align_up(cw, STRIDE_ALIGNMENT);
    let y_size = y_stride * h;
    let uv_size = uv_stride * ch;
    let total = y_size + uv_size * 2;
    (vec![0; total], y_stride, uv_stride)
}

pub(crate) fn fill_yuv420p_black(buf: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let y_stride = align_up(w, STRIDE_ALIGNMENT);
    let uv_stride = align_up(cw, STRIDE_ALIGNMENT);
    let y_size = y_stride * h;
    let uv_size = uv_stride * ch;
    buf[..y_size].fill(16);
    buf[y_size..y_size + uv_size].fill(128);
    buf[y_size + uv_size..y_size + uv_size * 2].fill(128);
}

pub(crate) fn build_yuv420p_image(
    buf: Vec<u8>,
    width: u32,
    height: u32,
    y_stride: usize,
    uv_stride: usize,
) -> Image {
    let h = height as usize;
    let ch = h.div_ceil(2);
    let y_size = y_stride * h;
    let uv_size = uv_stride * ch;
    let handle = BufferHandle::from_host_bytes(avcodec::core::utils::next_buffer_id(), buf);
    let mut image = Image::new(ImageInfo::Yuv420p, width, height, handle);
    image.layout = SampleLayout::Planar;
    image.set_plane(
        0,
        ImagePlane {
            offset: 0,
            stride: y_stride,
            len: y_size,
        },
    );
    image.set_plane(
        1,
        ImagePlane {
            offset: y_size,
            stride: uv_stride,
            len: uv_size,
        },
    );
    image.set_plane(
        2,
        ImagePlane {
            offset: y_size + uv_size,
            stride: uv_stride,
            len: uv_size,
        },
    );
    image
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn composite_tile(
    canvas_buf: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    y_stride: usize,
    uv_stride: usize,
    tile: &Image,
    cell: &MosaicCell,
    cell_width: u32,
    cell_height: u32,
) -> avcodec::core::AvResult<()> {
    let dst_x = cell.column * cell_width;
    let dst_y = cell.row * cell_height;
    if dst_x + cell_width > canvas_width || dst_y + cell_height > canvas_height {
        return Ok(());
    }

    let visible = tile.visible;
    let copy_w = visible.width.min(cell_width);
    let copy_h = visible.height.min(cell_height);
    if copy_w == 0 || copy_h == 0 {
        return Ok(());
    }

    let tile_y = tile
        .plane_host_bytes(0)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_u = tile
        .plane_host_bytes(1)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_v = tile
        .plane_host_bytes(2)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;

    let y_plane = tile.planes[0].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let u_plane = tile.planes[1].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let v_plane = tile.planes[2].ok_or(avcodec::core::AvError::InvalidArgument)?;

    let y_src_start = visible.y as usize * y_plane.stride + visible.x as usize;
    let y_dst_start = dst_y as usize * y_stride + dst_x as usize;
    for row in 0..copy_h as usize {
        let src = y_src_start + row * y_plane.stride;
        let dst = y_dst_start + row * y_stride;
        canvas_buf[dst..dst + copy_w as usize].copy_from_slice(&tile_y[src..src + copy_w as usize]);
    }

    let copy_cw = (copy_w as usize).div_ceil(2);
    let copy_ch = (copy_h as usize).div_ceil(2);
    let canvas_h = canvas_height as usize;
    let ch = canvas_h.div_ceil(2);
    let y_size = y_stride * canvas_h;
    let uv_size = uv_stride * ch;

    let u_src_start = (visible.y as usize / 2) * u_plane.stride + (visible.x as usize / 2);
    let u_dst_start = y_size + (dst_y as usize / 2) * uv_stride + (dst_x as usize / 2);
    for row in 0..copy_ch {
        let src = u_src_start + row * u_plane.stride;
        let dst = u_dst_start + row * uv_stride;
        canvas_buf[dst..dst + copy_cw].copy_from_slice(&tile_u[src..src + copy_cw]);
    }

    let v_src_start = (visible.y as usize / 2) * v_plane.stride + (visible.x as usize / 2);
    let v_dst_start = y_size + uv_size + (dst_y as usize / 2) * uv_stride + (dst_x as usize / 2);
    for row in 0..copy_ch {
        let src = v_src_start + row * v_plane.stride;
        let dst = v_dst_start + row * uv_stride;
        canvas_buf[dst..dst + copy_cw].copy_from_slice(&tile_v[src..src + copy_cw]);
    }

    Ok(())
}

pub(crate) fn compute_tile_geometry(
    image: &Image,
    cell_w: u32,
    cell_h: u32,
    fit: FitMode,
) -> (Rect, u32, u32) {
    let src_x = image.visible.x;
    let src_y = image.visible.y;
    let src_w = image.visible.width;
    let src_h = image.visible.height;
    let max_w = image.coded_width;
    let max_h = image.coded_height;

    match fit {
        FitMode::Stretch => {
            let crop = centered_even_rect(src_x, src_y, src_w, src_h, max_w, max_h);
            (crop, even_dim(cell_w).max(2), even_dim(cell_h).max(2))
        }
        FitMode::Cover => {
            let cell_aspect = cell_w as f64 / cell_h.max(1) as f64;
            let src_aspect = src_w as f64 / src_h.max(1) as f64;
            let (crop_w, crop_h) = if src_aspect > cell_aspect {
                let h = src_h;
                let w = ((src_h as f64 * cell_aspect) as u32).max(1);
                (w, h)
            } else {
                let w = src_w;
                let h = ((src_w as f64 / cell_aspect) as u32).max(1);
                (w, h)
            };
            let crop = centered_even_rect(src_x, src_y, crop_w, crop_h, max_w, max_h);
            (crop, even_dim(cell_w).max(2), even_dim(cell_h).max(2))
        }
        FitMode::Contain => {
            let scale =
                (cell_w as f64 / src_w.max(1) as f64).min(cell_h as f64 / src_h.max(1) as f64);
            let content_w = ((src_w as f64 * scale) as u32).max(1);
            let content_h = ((src_h as f64 * scale) as u32).max(1);
            let crop = centered_even_rect(src_x, src_y, src_w, src_h, max_w, max_h);
            (crop, even_dim(content_w).max(2), even_dim(content_h).max(2))
        }
    }
}

pub(crate) fn pad_yuv420p_to_cell(
    content: Image,
    cell_w: u32,
    cell_h: u32,
) -> avcodec::core::AvResult<Image> {
    let content_w = content.visible.width;
    let content_h = content.visible.height;
    if content_w == cell_w && content_h == cell_h {
        return Ok(content);
    }

    let cw = cell_w as usize;
    let ch = cell_h as usize;
    let uv_h = ch.div_ceil(2);
    let y_len = cw * ch;
    let uv_len = (cw / 2) * uv_h;
    let total = y_len + uv_len * 2;
    let mut buf = avcodec::core::buffer::allocate_host_vec(total);

    for y in &mut buf[0..y_len] {
        *y = 16;
    }
    for uv in &mut buf[y_len..y_len + uv_len * 2] {
        *uv = 128;
    }

    let off_x = ((cell_w - content_w) / 2) & !1;
    let off_y = ((cell_h - content_h) / 2) & !1;

    let y_plane = content.planes[0].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let u_plane = content.planes[1].ok_or(avcodec::core::AvError::InvalidArgument)?;
    let v_plane = content.planes[2].ok_or(avcodec::core::AvError::InvalidArgument)?;

    let tile_y = content
        .plane_host_bytes(0)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_u = content
        .plane_host_bytes(1)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;
    let tile_v = content
        .plane_host_bytes(2)?
        .ok_or(avcodec::core::AvError::InvalidArgument)?;

    let y_src_start = content.visible.y as usize * y_plane.stride + content.visible.x as usize;
    for row in 0..content_h as usize {
        let src = y_src_start + row * y_plane.stride;
        let dst = (off_y as usize + row) * cw + off_x as usize;
        buf[dst..dst + content_w as usize].copy_from_slice(&tile_y[src..src + content_w as usize]);
    }

    let content_uv_w = (content_w / 2) as usize;
    let content_uv_h = (content_h / 2) as usize;
    let uv_off_x = (off_x / 2) as usize;
    let uv_off_y = (off_y / 2) as usize;
    let uv_stride = cw / 2;

    let uv_src_start =
        (content.visible.y as usize / 2) * u_plane.stride + (content.visible.x as usize / 2);
    for row in 0..content_uv_h {
        let src = uv_src_start + row * u_plane.stride;
        let dst_u = y_len + (uv_off_y + row) * uv_stride + uv_off_x;
        buf[dst_u..dst_u + content_uv_w].copy_from_slice(&tile_u[src..src + content_uv_w]);
        let dst_v = y_len + uv_len + (uv_off_y + row) * uv_stride + uv_off_x;
        let v_src = (content.visible.y as usize / 2) * v_plane.stride
            + (content.visible.x as usize / 2)
            + row * v_plane.stride;
        buf[dst_v..dst_v + content_uv_w].copy_from_slice(&tile_v[v_src..v_src + content_uv_w]);
    }

    let handle = BufferHandle::from_host_bytes(avcodec::core::utils::next_buffer_id(), buf);
    let mut image =
        Image::new(ImageInfo::Yuv420p, cell_w, cell_h, handle).with_layout(SampleLayout::Planar);
    image.set_plane(
        0,
        ImagePlane {
            offset: 0,
            stride: cw,
            len: y_len,
        },
    );
    image.set_plane(
        1,
        ImagePlane {
            offset: y_len,
            stride: uv_stride,
            len: uv_len,
        },
    );
    image.set_plane(
        2,
        ImagePlane {
            offset: y_len + uv_len,
            stride: uv_stride,
            len: uv_len,
        },
    );
    Ok(image)
}

fn even_dim(v: u32) -> u32 {
    v & !1
}

fn centered_even_rect(x: u32, y: u32, mut w: u32, mut h: u32, max_w: u32, max_h: u32) -> Rect {
    w = even_dim(w).max(2);
    h = even_dim(h).max(2);
    if w > max_w {
        w = even_dim(max_w).max(2);
    }
    if h > max_h {
        h = even_dim(max_h).max(2);
    }
    let mut cx = x + (max_w.saturating_sub(w)) / 2;
    let mut cy = y + (max_h.saturating_sub(h)) / 2;
    if cx + w > max_w {
        cx = max_w.saturating_sub(w);
    }
    if cy + h > max_h {
        cy = max_h.saturating_sub(h);
    }
    cx &= !1;
    cy &= !1;
    Rect {
        x: cx,
        y: cy,
        width: w,
        height: h,
    }
}
