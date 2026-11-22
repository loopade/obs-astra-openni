"""直接调 OpenNI2 的 C API 抓一帧 Astra 的彩色流存成 BMP（不经过 OBS）。

用法：
    set ASTRA_OPENNI2_PATH=C:\\path\\to\\OpenNI2.dll
    python capture_example.py [输出文件.bmp] [设备序号]

OpenNI2.dll 的查找顺序：第一个命令行参数之后的环境变量 → 当前目录 → PATH。
列出的设备按 OpenNI 枚举顺序编号，不指定序号时用第 0 台。

注意：同一台相机不能被两个进程同时打开，抓帧前先关掉 OBS 里的 Astra 源。
"""
import ctypes
import os
import struct
import sys

ONI_SENSOR_COLOR = 2
ONI_STREAM_PROPERTY_VIDEO_MODE = 3
PIXEL_RGB888 = 200
PIXEL_YUYV = (201, 205)
ONI_MAX_STR_LEN = 256


class VideoMode(ctypes.Structure):
    _fields_ = [
        ("pixelFormat", ctypes.c_int),
        ("resX", ctypes.c_int),
        ("resY", ctypes.c_int),
        ("fps", ctypes.c_int),
    ]


class Frame(ctypes.Structure):
    _fields_ = [
        ("dataSize", ctypes.c_int),
        ("data", ctypes.c_void_p),
        ("sensorType", ctypes.c_int),
        ("timestamp", ctypes.c_uint64),
        ("frameIndex", ctypes.c_int),
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("videoMode", VideoMode),
        ("croppingEnabled", ctypes.c_int),
        ("cropOriginX", ctypes.c_int),
        ("cropOriginY", ctypes.c_int),
        ("stride", ctypes.c_int),
    ]


class DeviceInfo(ctypes.Structure):
    _fields_ = [
        ("uri", ctypes.c_char * ONI_MAX_STR_LEN),
        ("vendor", ctypes.c_char * ONI_MAX_STR_LEN),
        ("name", ctypes.c_char * ONI_MAX_STR_LEN),
        ("serialNumber", ctypes.c_char * ONI_MAX_STR_LEN),
        ("usbVendorId", ctypes.c_uint16),
        ("usbProductId", ctypes.c_uint16),
    ]


def load_openni():
    candidates = []
    env = os.environ.get("ASTRA_OPENNI2_PATH")
    if env:
        candidates.append(env)
    candidates += ["OpenNI2.dll", os.path.join(os.getcwd(), "OpenNI2.dll")]
    for c in candidates:
        try:
            dll = ctypes.CDLL(c)
        except OSError:
            continue
        print(f"OpenNI2: {c}")
        return dll
    sys.exit("OpenNI2.dll not found; set ASTRA_OPENNI2_PATH to its full path")


def bind(dll):
    dll.oniInitialize.argtypes = [ctypes.c_int]
    dll.oniShutdown.argtypes = []
    dll.oniGetDeviceList.argtypes = [
        ctypes.POINTER(ctypes.POINTER(DeviceInfo)),
        ctypes.POINTER(ctypes.c_int),
    ]
    dll.oniReleaseDeviceList.argtypes = [ctypes.POINTER(DeviceInfo)]
    dll.oniDeviceOpen.argtypes = [ctypes.c_char_p, ctypes.POINTER(ctypes.c_void_p)]
    dll.oniDeviceClose.argtypes = [ctypes.c_void_p]
    dll.oniDeviceCreateStream.argtypes = [
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.POINTER(ctypes.c_void_p),
    ]
    dll.oniStreamSetProperty.argtypes = [
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_void_p,
        ctypes.c_int,
    ]
    dll.oniStreamStart.argtypes = [ctypes.c_void_p]
    dll.oniStreamStop.argtypes = [ctypes.c_void_p]
    dll.oniStreamDestroy.argtypes = [ctypes.c_void_p]
    dll.oniStreamReadFrame.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.POINTER(Frame)),
        ctypes.c_int,
    ]
    dll.oniFrameRelease.argtypes = [ctypes.POINTER(Frame)]


def list_devices(dll):
    lst = ctypes.POINTER(DeviceInfo)()
    count = ctypes.c_int(0)
    if dll.oniGetDeviceList(ctypes.byref(lst), ctypes.byref(count)) != 0 or count.value <= 0:
        sys.exit("no OpenNI device found (check USB / camera driver)")
    devs = []
    for i in range(count.value):
        d = lst[i]
        devs.append((d.uri, d.name.decode(errors="replace"), d.usbVendorId, d.usbProductId))
    # 拷完再释放，避免 use-after-free
    dll.oniReleaseDeviceList(lst)
    for i, (uri, name, vid, pid) in enumerate(devs):
        print(f"[{i}] {name}  vid={vid:04x} pid={pid:04x}\n    {uri.decode(errors='replace')}")
    return devs


def rgb_to_bmp(rgb: bytes, w: int, h: int, flip_x: bool = True) -> bytes:
    """RGB888 -> 24bit BMP，顺便把结构光默认的水平镜像翻回来。"""
    bgr = bytearray(w * h * 3)
    for y in range(h):
        srow = y * w * 3
        drow = y * w * 3
        for x in range(w):
            sx = (w - 1 - x) if flip_x else x
            s = srow + sx * 3
            d = drow + x * 3
            bgr[d] = rgb[s + 2]
            bgr[d + 1] = rgb[s + 1]
            bgr[d + 2] = rgb[s]
    stride = (w * 3 + 3) & ~3
    size = 54 + stride * h
    bmp = bytearray(size)
    bmp[0:2] = b"BM"
    struct.pack_into("<I", bmp, 2, size)
    struct.pack_into("<I", bmp, 10, 54)
    struct.pack_into("<i", bmp, 14, 40)
    struct.pack_into("<ii", bmp, 18, w, h)
    struct.pack_into("<HH", bmp, 26, 1, 24)
    for y in range(h):
        s = (h - 1 - y) * w * 3
        d = 54 + y * stride
        bmp[d:d + w * 3] = bgr[s:s + w * 3]
    return bytes(bmp)


def capture(dll, uri: bytes, out_path: str, w: int = 640, h: int = 480) -> bool:
    dev = ctypes.c_void_p()
    if dll.oniDeviceOpen(uri, ctypes.byref(dev)) != 0:
        print("device open failed")
        return False
    stream = ctypes.c_void_p()
    if dll.oniDeviceCreateStream(dev, ONI_SENSOR_COLOR, ctypes.byref(stream)) != 0:
        print("color stream not available on this device "
              "(Astra Pro Plus exposes RGB as a separate UVC camera instead)")
        dll.oniDeviceClose(dev)
        return False

    vm = VideoMode(PIXEL_RGB888, w, h, 30)
    rc = dll.oniStreamSetProperty(stream, ONI_STREAM_PROPERTY_VIDEO_MODE,
                                  ctypes.byref(vm), ctypes.sizeof(vm))
    print("RGB888 mode rc =", rc)
    dll.oniStreamStart(stream)

    saved = False
    for i in range(60):
        fptr = ctypes.POINTER(Frame)()
        if dll.oniStreamReadFrame(stream, ctypes.byref(fptr), 2000) != 0:
            continue
        f = fptr.contents
        # 前几帧自动曝光还没稳定，丢掉
        if i >= 15 and f.videoMode.pixelFormat == PIXEL_RGB888:
            print(f"frame {f.width}x{f.height} fmt={f.videoMode.pixelFormat} bytes={f.dataSize}")
            rgb = ctypes.string_at(f.data, f.dataSize)
            with open(out_path, "wb") as fh:
                fh.write(rgb_to_bmp(rgb, f.width, f.height))
            print("saved:", out_path)
            saved = True
        elif i >= 15:
            print(f"unexpected pixel format {f.videoMode.pixelFormat} (expected {PIXEL_RGB888})")
        dll.oniFrameRelease(fptr)
        if saved:
            break

    dll.oniStreamStop(stream)
    dll.oniStreamDestroy(stream)
    dll.oniDeviceClose(dev)
    return saved


def main() -> None:
    args = [a for a in sys.argv[1:]]
    out_path = args[0] if args else "capture.bmp"
    index = int(args[1]) if len(args) > 1 else 0

    dll = load_openni()
    bind(dll)
    if dll.oniInitialize(0) != 0:
        sys.exit("oniInitialize failed")
    try:
        devs = list_devices(dll)
        if index >= len(devs):
            sys.exit(f"device index {index} out of range (found {len(devs)})")
        ok = capture(dll, devs[index][0], out_path)
    finally:
        dll.oniShutdown()
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
