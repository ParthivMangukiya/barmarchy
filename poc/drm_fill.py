#!/usr/bin/env python3
"""M1 Touch Bar DRM fill PoC — replicates tiny-dfr's display.rs path.

Legacy SETCRTC, dumb buffer 64x2008 XRGB8888 exactly like tiny-dfr,
asymmetric solid pattern so the viewer can report orientation.
Struct sizes counted from /usr/include/drm/drm_mode.h.
"""
import ctypes
import fcntl
import mmap
import os
import struct
import sys
import time

C = ctypes
CARD = "/dev/dri/card2"
W, H = 64, 2008  # tiny-dfr uses 64-wide dumb buffer for the 60-wide mode


def IOWR(nr, size):
    return (3 << 30) | (size << 16) | (0x64 << 8) | nr


# drm_mode_card_res = 4x u64 + 8x u32 = 64
IOCTL_GETRES = IOWR(0xA0, 64)
# drm_mode_crtc = u64 + 7x u32 + modeinfo(68) = 104
IOCTL_SETCRTC = IOWR(0xA2, 104)
# drm_mode_get_connector = 4x u64 + 12x u32 = 80
IOCTL_GETCONN = IOWR(0xA7, 80)
# drm_mode_fb_cmd2 = 5x u32 + 3x u32[4] + u64[4] = 100
IOCTL_ADDFB2 = IOWR(0xB8, 100)
# drm_mode_fb_dirty_cmd = 4x u32 + u64 = 24
IOCTL_DIRTYFB = IOWR(0xB1, 24)
# drm_mode_create_dumb = 6x u32 + u64 = 32
IOCTL_CREATE_DUMB = IOWR(0xB2, 32)
# drm_mode_map_dumb = 2x u32 + u64 = 16
IOCTL_MAP_DUMB = IOWR(0xB3, 16)

DRM_FORMAT_XRGB8888 = 0x34325258
CONNECTED = 1
RES_FMT = "QQQQIIIIIIII"
CONN_FMT = "QQQQ" + "I" * 12
# u32 field order: count_modes, count_props, count_encoders, encoder_id,
# connector_id, connector_type, connector_type_id, connection,
# mm_width, mm_height, subpixel, pad


def main():
    hold = float(sys.argv[sys.argv.index("--hold") + 1]) if "--hold" in sys.argv else 6.0
    fd = os.open(CARD, os.O_RDWR)

    # 1. resources (two-step: counts first, then fill)
    res = bytearray(64)
    fcntl.ioctl(fd, IOCTL_GETRES, res)
    parts = struct.unpack(RES_FMT, res)
    (n_fb, n_crtc, n_conn, n_enc) = parts[4:8]
    crtc_arr = (C.c_uint32 * n_crtc)()
    conn_arr = (C.c_uint32 * n_conn)()
    enc_arr = (C.c_uint32 * n_enc)()
    fb_arr = (C.c_uint32 * n_fb)()
    struct.pack_into("QQQQ", res, 0, C.addressof(fb_arr),
                     C.addressof(crtc_arr), C.addressof(conn_arr),
                     C.addressof(enc_arr))
    fcntl.ioctl(fd, IOCTL_GETRES, res)
    crtc_id = crtc_arr[0]
    print(f"crtcs={list(crtc_arr)} conns={list(conn_arr)}", flush=True)

    # 2. find connected connector + its first mode (two-step for modes)
    chosen, mode_bytes = None, None
    for cid in conn_arr:
        c = bytearray(80)
        struct.pack_into("I", c, 48, cid)  # connector_id is u32 #4
        fcntl.ioctl(fd, IOCTL_GETCONN, c)
        vals = struct.unpack(CONN_FMT, c)
        n_modes = vals[4]
        connection = vals[11]
        if connection != CONNECTED or n_modes == 0:
            continue
        mbuf = (C.c_char * (68 * n_modes))()
        struct.pack_into("Q", c, 8, C.addressof(mbuf))  # modes_ptr
        struct.pack_into("II", c, 36, 0, 0)  # count_props=0, count_encoders=0
        fcntl.ioctl(fd, IOCTL_GETCONN, c)
        mode_bytes = bytes(mbuf[:68])
        (clk, hd, _, _, _, _, vd) = struct.unpack("IHHHHHH", mode_bytes[:16])
        print(f"conn={cid} connected modes={n_modes} first={hd}x{vd}", flush=True)
        chosen = cid
        break
    if chosen is None:
        print("no connected connector", flush=True)
        return 1

    # 3. dumb buffer + fb
    dumb = bytearray(struct.pack("IIIIIIQ", H, W, 32, 0, 0, 0, 0))
    fcntl.ioctl(fd, IOCTL_CREATE_DUMB, dumb)
    (_, _, _, _, handle, pitch, size) = struct.unpack("IIIIIIQ", dumb)
    print(f"dumb handle={handle} pitch={pitch} size={size}", flush=True)

    fb2 = bytearray(100)
    struct.pack_into("IIIII", fb2, 0, 0, W, H, DRM_FORMAT_XRGB8888, 0)
    struct.pack_into("IIII", fb2, 20, handle, 0, 0, 0)   # handles
    struct.pack_into("IIII", fb2, 36, pitch, 0, 0, 0)    # pitches
    # offsets (52) + modifier (68) stay zero
    fcntl.ioctl(fd, IOCTL_ADDFB2, fb2)
    fb_id = struct.unpack("I", fb2[:4])[0]
    print(f"fb_id={fb_id}", flush=True)

    # 4. setcrtc: ptr@0 count@8 crtc@12 fb@16 x@20 y@24 gamma@28 valid@32 mode@36
    conn_list = (C.c_uint32 * 1)(chosen)
    crtc = bytearray(104)
    struct.pack_into("Q", crtc, 0, C.addressof(conn_list))
    struct.pack_into("IIIIIII", crtc, 8, 1, crtc_id, fb_id, 0, 0, 0, 1)
    crtc[36:36 + 68] = mode_bytes
    fcntl.ioctl(fd, IOCTL_SETCRTC, crtc)
    print("setcrtc ok", flush=True)

    # 5. map + fill asymmetric pattern
    md = bytearray(struct.pack("IIQ", handle, 0, 0))
    fcntl.ioctl(fd, IOCTL_MAP_DUMB, md)
    (_, _, offset) = struct.unpack("IIQ", md)
    print(f"map offset={offset:#x}", flush=True)
    # NOTE: must mmap the SAME fd (DRM map offsets are per-open-file)
    mm = mmap.mmap(fd, size, offset=offset)
    RED, BLUE, WHITE = 0x00FF0000, 0x000000FF, 0x00FFFFFF
    row_white = struct.pack(f"{W}I", *([WHITE] * W))
    row_blue_red = struct.pack(
        f"{W}I", *([BLUE] * 20 + [RED] * (W - 20)))
    for y in range(H):
        off = y * W * 4
        mm[off:off + W * 4] = row_white if y < 200 else row_blue_red
    # NOTE: no mm.flush() — msync is invalid on dumb buffers;
    # writes are visible through the shared mapping, DIRTYFB notifies.

    # 6. dirty full fb (clip rect: x1,y1,x2,y2 as u16)
    clip = (C.c_char * 8).from_buffer_copy(struct.pack("HHHH", 0, 0, W, H))
    dirty = bytearray(struct.pack("IIIIQ", fb_id, 0, 0, 1,
                                  C.addressof(clip)))
    fcntl.ioctl(fd, IOCTL_DIRTYFB, dirty)
    print(f"dirty ok, holding {hold}s", flush=True)
    sys.stdout.flush()
    time.sleep(hold)
    print("done (leaving framebuffer up)", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
