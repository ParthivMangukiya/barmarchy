//! DRM dumb-buffer backend (1:1 port of the Python ioctl path).
//!
//! Opens /dev/dri/card2, picks the connected connector, creates a
//! 64x2008 XRGB8888 dumb buffer, modesets, and mmaps it for blits.

use std::fs::File;
use std::os::unix::io::AsRawFd;

pub const CARD: &str = "/dev/dri/card2";
pub const W: i32 = 64;
pub const H: i32 = 2008;
pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;

// drmMode ioctl numbers (DRM_IOCTL_BASE 'd'=0x64)
fn iowr(nr: u32, size: usize) -> u64 {
    (3u64 << 30) | ((size as u64) << 16) | (0x64u64 << 8) | nr as u64
}

fn ioctl_getresources() -> u64 {
    iowr(0xA0, 64)
}
fn ioctl_setcrtc() -> u64 {
    iowr(0xA2, 104)
}
fn ioctl_getconnector() -> u64 {
    iowr(0xA7, 80)
}
fn ioctl_addfb2() -> u64 {
    iowr(0xB8, 100)
}
fn ioctl_dirtyfb() -> u64 {
    iowr(0xB1, 24)
}
fn ioctl_create_dumb() -> u64 {
    iowr(0xB2, 32)
}
fn ioctl_map_dumb() -> u64 {
    iowr(0xB3, 16)
}

fn ioctl(fd: i32, req: u64, arg: *mut std::ffi::c_void) -> std::io::Result<()> {
    let r = unsafe { libc::ioctl(fd, req as _, arg) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Res {
    fb_ptr: u64,
    crtc_ptr: u64,
    conn_ptr: u64,
    enc_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_conns: u32,
    count_encs: u32,
    min_w: u32,
    min_h: u32,
    max_w: u32,
    max_h: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ModeInfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    type_: u32,
    name: [u8; 32],
}

/// Matches kernel struct drm_mode_get_connector exactly (80 bytes):
/// encoders_ptr, modes_ptr, props_ptr, prop_values_ptr, then 12xu32.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Conn {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AddFb2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SetCrtc {
    conns_ptr: u64,
    count: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Clip {
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
}

#[repr(C)]
struct Dirty {
    fb_id: u32,
    flags: u32,
    color_index: u32,
    num_clips: u32,
    clips_ptr: u64,
}

pub struct DrmBackend {
    file: File,
    map: Option<memmap2::MmapMut>,
    fb_id: u32,
    pitch: u32,
    size: usize,
}

impl DrmBackend {
    pub fn open() -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(CARD)?;
        Ok(Self {
            file,
            map: None,
            fb_id: 0,
            pitch: 0,
            size: 0,
        })
    }

    pub fn modeset(&mut self) -> std::io::Result<()> {
        let fd = self.file.as_raw_fd();
        // pass 1: counts
        let mut res = Res::default();
        ioctl(fd, ioctl_getresources(), &mut res as *mut _ as *mut _)?;
        let (n_crtc, n_conn) = (res.count_crtcs as usize, res.count_conns as usize);
        // NB: never hand the kernel a dangling pointer from a 0-len Vec —
        // allocate at least 1 element (counts still govern access).
        let mut crtcs = vec![0u32; n_crtc.max(1)];
        let mut conns = vec![0u32; n_conn.max(1)];
        let mut encs = vec![0u32; (res.count_encs as usize).max(1)];
        let mut fbs = vec![0u32; (res.count_fbs as usize).max(1)];
        res.crtc_ptr = crtcs.as_mut_ptr() as u64;
        res.conn_ptr = conns.as_mut_ptr() as u64;
        res.enc_ptr = encs.as_mut_ptr() as u64;
        res.fb_ptr = fbs.as_mut_ptr() as u64;
        ioctl(fd, ioctl_getresources(), &mut res as *mut _ as *mut _)?;
        let crtc_id = crtcs[0];

        let mut chosen: Option<(u32, ModeInfo)> = None;
        for cid in conns.iter().take(n_conn) {
            let mut c = Conn::default();
            c.connector_id = *cid;
            ioctl(fd, ioctl_getconnector(), &mut c as *mut _ as *mut _)?;
            if c.connection != 1 || c.count_modes == 0 {
                continue;
            }
            let mut modes = vec![ModeInfo::default(); c.count_modes as usize];
            c.modes_ptr = modes.as_mut_ptr() as u64;
            // zero property/encoder counts (ptrs are NULL) — mirrors python
            c.count_props = 0;
            c.count_encoders = 0;
            ioctl(fd, ioctl_getconnector(), &mut c as *mut _ as *mut _)?;
            chosen = Some((*cid, modes[0]));
            break;
        }
        let (conn_id, mode) = chosen.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no connected connector")
        })?;

        let mut dumb = CreateDumb {
            height: H as u32,
            width: W as u32,
            bpp: 32,
            ..Default::default()
        };
        ioctl(fd, ioctl_create_dumb(), &mut dumb as *mut _ as *mut _)?;

        let mut fb = AddFb2 {
            width: W as u32,
            height: H as u32,
            pixel_format: DRM_FORMAT_XRGB8888,
            ..Default::default()
        };
        fb.handles[0] = dumb.handle;
        fb.pitches[0] = dumb.pitch;
        ioctl(fd, ioctl_addfb2(), &mut fb as *mut _ as *mut _)?;

        let conn_list = [conn_id];
        let mut crtc = SetCrtc {
            conns_ptr: conn_list.as_ptr() as u64,
            count: 1,
            crtc_id,
            fb_id: fb.fb_id,
            mode_valid: 1,
            mode,
            ..Default::default()
        };
        ioctl(fd, ioctl_setcrtc(), &mut crtc as *mut _ as *mut _)?;

        let mut md = MapDumb {
            handle: dumb.handle,
            ..Default::default()
        };
        ioctl(fd, ioctl_map_dumb(), &mut md as *mut _ as *mut _)?;

        let map = unsafe {
            memmap2::MmapOptions::new()
                .len(dumb.size as usize)
                .offset(md.offset)
                .map_mut(&self.file)?
        };
        self.fb_id = fb.fb_id;
        self.pitch = dumb.pitch;
        self.size = dumb.size as usize;
        self.map = Some(map);
        eprintln!(
            "modeset ok: conn={} fb={} pitch={} size={}",
            conn_id, self.fb_id, self.pitch, self.size
        );
        Ok(())
    }

    pub fn blit(&mut self, rgba: &[u8], stride: usize) -> std::io::Result<()> {
        let map = self
            .map
            .as_mut()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotConnected, "no mmap"))?;
        // cairo stride for 64px ARGB32 = 256; dumb pitch is usually 256 too.
        // Copy row by row to be safe against pitch mismatch.
        let rows = H as usize;
        let copy_w = (stride as u32).min(self.pitch) as usize;
        for y in 0..rows {
            let src = &rgba[y * stride..y * stride + copy_w];
            let dst_off = y * self.pitch as usize;
            map[dst_off..dst_off + copy_w].copy_from_slice(src);
        }
        // NB: no msync here — the DIRTYFB ioctl below is the flush
        // (msync on the dumb-buffer mapping returns EINVAL).
        let clip = Clip {
            x1: 0,
            y1: 0,
            x2: W as u16,
            y2: H as u16,
        };
        let mut dirty = Dirty {
            fb_id: self.fb_id,
            flags: 0,
            color_index: 0,
            num_clips: 1,
            clips_ptr: &clip as *const _ as u64,
        };
        ioctl(
            self.file.as_raw_fd(),
            ioctl_dirtyfb(),
            &mut dirty as *mut _ as *mut _,
        )?;
        Ok(())
    }
}
