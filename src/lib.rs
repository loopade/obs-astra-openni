//! 最小 OBS 插件：把 Orbbec Astra 系列（OpenNI 协议，PID 0401 等）的彩色流作为异步视频源。
//!
//! 设计要点：
//! - 不链接 libobs import lib，运行期从已加载的 obs.dll 取 `obs_register_source_s` /
//!   `obs_source_output_video`，因此编译产物只是一个 DLL，没有任何构建期依赖。
//! - OpenNI2.dll 也运行期加载，搜索顺序见 `openni::load`。
//! - 结构体布局严格对齐 obs-studio 33.0.3 的 libobs/obs.h 与 obs-source.h。

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

// ---------------------------------------------------------------- Win32

extern "system" {
    fn LoadLibraryA(name: *const c_char) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
    fn GetModuleHandleExA(flags: u32, name: *const c_char, module: *mut *mut c_void) -> i32;
    fn GetModuleFileNameA(module: *mut c_void, buf: *mut c_char, size: u32) -> u32;
}

/// 本插件 DLL 所在目录，例如 ...\plugins\astra-openni\bin\64bit
unsafe fn own_dir() -> Option<String> {
    let mut module: *mut c_void = std::ptr::null_mut();
    // GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN
    if GetModuleHandleExA(0x4 | 0x1, own_dir as *const c_char, &mut module) == 0 {
        return None;
    }
    let mut buf = [0i8; 1024];
    let n = GetModuleFileNameA(module, buf.as_mut_ptr(), buf.len() as u32);
    if n == 0 {
        return None;
    }
    let path = String::from_utf8_lossy(&{
        let mut v = Vec::new();
        for i in 0..n as usize {
            v.push(buf[i] as u8);
        }
        v
    })
    .to_string();
    path.rfind('\\').map(|i| path[..i].to_string())
}

/// 追加一行到插件目录下的 astra-openni.log（诊断用）
fn alog(msg: &str) {
    use std::io::Write;
    let dir = unsafe { own_dir() };
    let Some(dir) = dir else { return };
    let path = format!("{dir}\\astra-openni.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", msg);
    }
}

unsafe fn cstr_to_string(s: &[c_char]) -> String {
    let bytes: Vec<u8> = s.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).to_string()
}

// ---------------------------------------------------------------- libobs ABI

const MAX_AV_PLANES: usize = 8;
const VIDEO_FORMAT_BGR3: c_int = 11;
const OBS_SOURCE_VIDEO: u32 = 1 << 0;
const OBS_SOURCE_ASYNC: u32 = 1 << 2;
const OBS_SOURCE_ASYNC_VIDEO: u32 = OBS_SOURCE_ASYNC | OBS_SOURCE_VIDEO;

#[repr(C)]
struct ObsSourceFrame {
    data: [*mut u8; MAX_AV_PLANES],
    linesize: [u32; MAX_AV_PLANES],
    width: u32,
    height: u32,
    timestamp: u64,
    format: c_int,
    color_matrix: [f32; 16],
    full_range: bool,
    max_luminance: u16,
    color_range_min: [f32; 3],
    color_range_max: [f32; 3],
    flip: bool,
    flags: u8,
    trc: u8,
    refs: i32,
    prev_frame: bool,
}

type FnGetName = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type FnCreate = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
type FnVoid = unsafe extern "C" fn(*mut c_void);
type FnDim = unsafe extern "C" fn(*mut c_void) -> u32;

#[repr(C)]
struct ObsSourceInfo {
    id: *const c_char,
    kind: c_int,
    output_flags: u32,
    get_name: Option<FnGetName>,
    create: Option<FnCreate>,
    destroy: Option<FnVoid>,
    get_width: Option<FnDim>,
    get_height: Option<FnDim>,
    get_defaults: Option<FnVoid>,
    get_properties: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    update: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    activate: Option<FnVoid>,
    deactivate: Option<FnVoid>,
    show: Option<FnVoid>,
    hide: Option<FnVoid>,
    video_tick: Option<unsafe extern "C" fn(*mut c_void, f32)>,
    video_render: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    filter_video: Option<unsafe extern "C" fn(*mut c_void, *mut ObsSourceFrame) -> *mut ObsSourceFrame>,
    filter_audio: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void>,
    enum_active_sources: Option<unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void)>,
    save: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    load: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    mouse_click: Option<unsafe extern "C" fn(*mut c_void, *const c_void, c_int, bool, u32)>,
    mouse_move: Option<unsafe extern "C" fn(*mut c_void, *const c_void, bool)>,
    mouse_wheel: Option<unsafe extern "C" fn(*mut c_void, *const c_void, c_int, c_int)>,
    focus: Option<unsafe extern "C" fn(*mut c_void, bool)>,
    key_click: Option<unsafe extern "C" fn(*mut c_void, *const c_void, bool)>,
    filter_remove: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    type_data: *mut c_void,
    free_type_data: Option<FnVoid>,
    audio_render: Option<
        unsafe extern "C" fn(*mut c_void, *mut u64, *mut c_void, u32, usize, usize) -> bool,
    >,
    enum_all_sources: Option<unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void)>,
    transition_start: Option<FnVoid>,
    transition_stop: Option<FnVoid>,
    get_defaults2: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    get_properties2: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void>,
    audio_mix: Option<unsafe extern "C" fn(*mut c_void, *mut u64, *mut c_void, usize, usize) -> bool>,
    icon_type: c_int,
    media_play_pause: Option<unsafe extern "C" fn(*mut c_void, bool)>,
    media_restart: Option<FnVoid>,
    media_stop: Option<FnVoid>,
    media_next: Option<FnVoid>,
    media_previous: Option<FnVoid>,
    media_get_duration: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    media_get_time: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    media_set_time: Option<unsafe extern "C" fn(*mut c_void, i64)>,
    media_get_state: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    version: u32,
    unversioned_id: *const c_char,
    missing_files: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void>,
    video_get_color_space: Option<unsafe extern "C" fn(*mut c_void, usize, *const c_int) -> c_int>,
    filter_add: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    get_dark_icon: Option<FnGetName>,
    get_light_icon: Option<FnGetName>,
}

impl ObsSourceInfo {
    const fn empty() -> Self {
        Self {
            id: std::ptr::null(),
            kind: 0,
            output_flags: 0,
            get_name: None,
            create: None,
            destroy: None,
            get_width: None,
            get_height: None,
            get_defaults: None,
            get_properties: None,
            update: None,
            activate: None,
            deactivate: None,
            show: None,
            hide: None,
            video_tick: None,
            video_render: None,
            filter_video: None,
            filter_audio: None,
            enum_active_sources: None,
            save: None,
            load: None,
            mouse_click: None,
            mouse_move: None,
            mouse_wheel: None,
            focus: None,
            key_click: None,
            filter_remove: None,
            type_data: std::ptr::null_mut(),
            free_type_data: None,
            audio_render: None,
            enum_all_sources: None,
            transition_start: None,
            transition_stop: None,
            get_defaults2: None,
            get_properties2: None,
            audio_mix: None,
            icon_type: 0,
            media_play_pause: None,
            media_restart: None,
            media_stop: None,
            media_next: None,
            media_previous: None,
            media_get_duration: None,
            media_get_time: None,
            media_set_time: None,
            media_get_state: None,
            version: 0,
            unversioned_id: std::ptr::null(),
            missing_files: None,
            video_get_color_space: None,
            filter_add: None,
            get_dark_icon: None,
            get_light_icon: None,
        }
    }
}

static OUTPUT_VIDEO: AtomicUsize = AtomicUsize::new(0);
static MODULE_PTR: AtomicUsize = AtomicUsize::new(0);
static LOGN: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------- OpenNI ABI

#[repr(C)]
#[derive(Clone, Copy)]
struct OniVideoMode {
    pixel_format: c_int,
    resolution_x: c_int,
    resolution_y: c_int,
    fps: c_int,
}

#[repr(C)]
struct OniFrame {
    data_size: c_int,
    data: *mut u8,
    sensor_type: c_int,
    timestamp: u64,
    frame_index: c_int,
    width: c_int,
    height: c_int,
    video_mode: OniVideoMode,
    cropping_enabled: c_int,
    crop_origin_x: c_int,
    crop_origin_y: c_int,
    stride: c_int,
}

#[repr(C)]
struct OniDeviceInfo {
    uri: [c_char; 256],
    vendor: [c_char; 256],
    name: [c_char; 256],
    serial_number: [c_char; 256],
    usb_vendor_id: u16,
    usb_product_id: u16,
}

const ONI_SENSOR_COLOR: c_int = 2;
const ONI_STREAM_PROPERTY_VIDEO_MODE: c_int = 3;
const PIXEL_RGB888: c_int = 200;
const PIXEL_YUYV: c_int = 201;

type FnDeviceCreateStream =
    unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_void) -> c_int;
type FnStreamSetProperty =
    unsafe extern "C" fn(*mut c_void, c_int, *const c_void, c_int) -> c_int;

struct Oni {
    initialize: unsafe extern "C" fn(c_int) -> c_int,
    shutdown: unsafe extern "C" fn() -> c_int,
    get_device_list:
        unsafe extern "C" fn(*mut *mut OniDeviceInfo, *mut c_int) -> c_int,
    release_device_list: unsafe extern "C" fn(*mut OniDeviceInfo) -> c_int,
    device_open: unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int,
    device_close: unsafe extern "C" fn(*mut c_void) -> c_int,
    device_create_stream: FnDeviceCreateStream,
    stream_set_property: FnStreamSetProperty,
    stream_start: unsafe extern "C" fn(*mut c_void) -> c_int,
    stream_stop: unsafe extern "C" fn(*mut c_void) -> c_int,
    stream_destroy: unsafe extern "C" fn(*mut c_void) -> c_int,
    stream_read_frame:
        unsafe extern "C" fn(*mut c_void, *mut *mut OniFrame, c_int) -> c_int,
    frame_release: unsafe extern "C" fn(*mut OniFrame),
}

unsafe fn sym(module: *mut c_void, name: &CStr) -> Option<*mut c_void> {
    let p = GetProcAddress(module, name.as_ptr());
    if p.is_null() {
        None
    } else {
        Some(p)
    }
}

unsafe fn load_oni() -> Option<Oni> {
    // 1) 环境变量指向完整路径；2) 系统搜索路径；3) 常见安装位置。
    let mut module: *mut c_void = std::ptr::null_mut();
    let env_path = std::env::var("ASTRA_OPENNI2_PATH").unwrap_or_default();
    let mut candidates: Vec<String> = Vec::new();
    if !env_path.is_empty() {
        candidates.push(env_path);
    }
    if let Some(dir) = own_dir() {
        candidates.push(format!("{dir}\\OpenNI2.dll"));
    }
    // 自包含安装：OpenNI2 运行库放在 ProgramData 的插件私有目录里
    if let Ok(pd) = std::env::var("ProgramData") {
        candidates.push(format!(
            "{pd}\\obs-studio\\plugins\\astra-openni\\bin\\64bit\\OpenNI2.dll"
        ));
    }
    candidates.push("OpenNI2.dll".to_string());
    candidates.push(r"C:\Program Files\OpenNI2\Tools\OpenNI2.dll".to_string());
    candidates.push(
        r"C:\Program Files (x86)\OpenNI2\Tools\OpenNI2.dll".to_string(),
    );

    for c in candidates {
        let mut buf: Vec<u8> = c.as_bytes().to_vec();
        buf.push(0);
        module = LoadLibraryA(buf.as_ptr() as *const c_char);
        if !module.is_null() {
            // 告诉 OpenNI 去哪里找 OpenNI2\Drivers（默认按 exe 目录推算，OBS 场景下会算错）
            if std::env::var("OPENNI2_REDIST").is_err() {
                if let Some(pos) = c.rfind('\\') {
                    std::env::set_var("OPENNI2_REDIST", &c[..pos]);
                }
            }
            break;
        }
    }
    if module.is_null() {
        return None;
    }

    macro_rules! grab {
        ($n:expr) => {{
            let name = CStr::from_bytes_with_nul($n).ok()?;
            std::mem::transmute::<*mut c_void, _>(sym(module, name)?)
        }};
    }

    Some(Oni {
        initialize: grab!(b"oniInitialize\0"),
        shutdown: grab!(b"oniShutdown\0"),
        get_device_list: grab!(b"oniGetDeviceList\0"),
        release_device_list: grab!(b"oniReleaseDeviceList\0"),
        device_open: grab!(b"oniDeviceOpen\0"),
        device_close: grab!(b"oniDeviceClose\0"),
        device_create_stream: grab!(b"oniDeviceCreateStream\0"),
        stream_set_property: grab!(b"oniStreamSetProperty\0"),
        stream_start: grab!(b"oniStreamStart\0"),
        stream_stop: grab!(b"oniStreamStop\0"),
        stream_destroy: grab!(b"oniStreamDestroy\0"),
        stream_read_frame: grab!(b"oniStreamReadFrame\0"),
        frame_release: grab!(b"oniFrameRelease\0"),
    })
}

// ---------------------------------------------------------------- 源实现

struct Ctx {
    source: *mut c_void,
    width: u32,
    height: u32,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

unsafe extern "C" fn get_name(_t: *mut c_void) -> *const c_char {
    b"Astra OpenNI Color\0".as_ptr() as *const c_char
}

unsafe extern "C" fn create(_settings: *mut c_void, source: *mut c_void) -> *mut c_void {
    let (w, h) = (640u32, 480u32);
    let stop = Arc::new(AtomicBool::new(false));
    let ctx = Box::new(Ctx {
        source,
        width: w,
        height: h,
        stop: stop.clone(),
        handle: None,
    });
    let raw = Box::into_raw(ctx);
    let src_addr = source as usize;
    let handle = thread::spawn(move || {
        unsafe { stream_loop(src_addr, w, h, stop) };
    });
    (*raw).handle = Some(handle);
    raw as *mut c_void
}

unsafe extern "C" fn destroy(data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let mut ctx = Box::from_raw(data as *mut Ctx);
    ctx.stop.store(true, Ordering::SeqCst);
    if let Some(h) = ctx.handle.take() {
        let _ = h.join();
    }
}

unsafe extern "C" fn get_width(data: *mut c_void) -> u32 {
    if data.is_null() {
        0
    } else {
        (*(data as *mut Ctx)).width
    }
}

unsafe extern "C" fn get_height(data: *mut c_void) -> u32 {
    if data.is_null() {
        0
    } else {
        (*(data as *mut Ctx)).height
    }
}

unsafe fn stream_loop(source: usize, width: u32, height: u32, stop: Arc<AtomicBool>) {
    let oni = match load_oni() {
        Some(v) => v,
        None => return,
    };
    let out_fn: Option<unsafe extern "C" fn(*mut c_void, *const ObsSourceFrame)> =
        std::mem::transmute(OUTPUT_VIDEO.load(Ordering::SeqCst));
    let out_fn = match out_fn {
        Some(f) => f,
        None => return,
    };

    if (oni.initialize)(0) != 0 {
        return;
    }

    let mut list: *mut OniDeviceInfo = std::ptr::null_mut();
    let mut count: c_int = 0;
    if (oni.get_device_list)(&mut list, &mut count) != 0 || count <= 0 || list.is_null() {
        alog("no device found");
        (oni.shutdown)();
        return;
    }
    // 先把 uri 拷进本地缓冲再释放列表，避免 use-after-free
    let mut uri_buf = [0i8; 256];
    uri_buf.copy_from_slice(&(*list).uri);
    (oni.release_device_list)(list);
    let uri: [c_char; 256] = uri_buf;
    alog(&format!("devices={} uri={}", count, cstr_to_string(&uri)));

    let mut dev: *mut c_void = std::ptr::null_mut();
    if (oni.device_open)(uri.as_ptr(), &mut dev) != 0 {
        alog("device open fail");
        (oni.shutdown)();
        return;
    }
    let mut stream: *mut c_void = std::ptr::null_mut();
    if (oni.device_create_stream)(dev, ONI_SENSOR_COLOR, &mut stream) != 0 {
        alog("color stream create fail");
        (oni.device_close)(dev);
        (oni.shutdown)();
        return;
    }

    let mode = OniVideoMode {
        pixel_format: PIXEL_RGB888,
        resolution_x: width as c_int,
        resolution_y: height as c_int,
        fps: 30,
    };
    if (oni.stream_set_property)(
        stream,
        ONI_STREAM_PROPERTY_VIDEO_MODE,
        &mode as *const OniVideoMode as *const c_void,
        std::mem::size_of::<OniVideoMode>() as c_int,
    ) != 0
    {
        alog("RGB888 mode rejected, fallback YUYV");
        // 退回 YUYV
        let fallback = OniVideoMode {
            pixel_format: PIXEL_YUYV,
            ..mode
        };
        let _ = (oni.stream_set_property)(
            stream,
            ONI_STREAM_PROPERTY_VIDEO_MODE,
            &fallback as *const OniVideoMode as *const c_void,
            std::mem::size_of::<OniVideoMode>() as c_int,
        );
    }

    if (oni.stream_start)(stream) != 0 {
        alog("stream start fail");
        (oni.stream_destroy)(stream);
        (oni.device_close)(dev);
        (oni.shutdown)();
        return;
    }
    alog("streaming started");

    let src = source as *mut c_void;
    let stride = (width * 3) as usize;
    let mut buf = vec![0u8; stride * height as usize];
    let mut logged = 0u32;
    let _ = logged;
    std::panic::set_hook(Box::new(|info| {
        alog(&format!("PANIC: {info}"));
    }));

    while !stop.load(Ordering::SeqCst) {
        let mut frame: *mut OniFrame = std::ptr::null_mut();
        if (oni.stream_read_frame)(stream, &mut frame, 200) != 0 || frame.is_null() {
            continue;
        }
        let f = &*frame;
        let fw = f.width as usize;
        let fh = f.height as usize;
        if fw == 0 || fh == 0 || fw * 3 > stride {
            if logged < 5 {
                logged += 1;
                alog(&format!(
                    "unexpected frame {}x{} fmt={}",
                    fw, fh, f.video_mode.pixel_format
                ));
            }
            (oni.frame_release)(frame);
            continue;
        }
        let bytes = std::slice::from_raw_parts(f.data, f.data_size as usize);
        match f.video_mode.pixel_format {
            PIXEL_RGB888 => {
                // RGB888 -> BGR3，同时水平镜像（结构光出图默认镜像）
                for y in 0..fh {
                    let srow = &bytes[y * fw * 3..(y + 1) * fw * 3];
                    let drow = &mut buf[y * stride..y * stride + fw * 3];
                    for x in 0..fw {
                        let s = &srow[(fw - 1 - x) * 3..(fw - 1 - x) * 3 + 3];
                        let d = &mut drow[x * 3..x * 3 + 3];
                        d[0] = s[2];
                        d[1] = s[1];
                        d[2] = s[0];
                    }
                }
            }
            PIXEL_YUYV => {
                yuyv_to_bgr(bytes, fw, fh, stride, &mut buf);
            }
            _ => {
                (oni.frame_release)(frame);
                continue;
            }
        }

        let mut of = ObsSourceFrame {
            data: [std::ptr::null_mut(); MAX_AV_PLANES],
            linesize: [0; MAX_AV_PLANES],
            width,
            height,
            timestamp: f.timestamp,
            format: VIDEO_FORMAT_BGR3,
            color_matrix: [0.0; 16],
            full_range: false,
            max_luminance: 0,
            color_range_min: [0.0; 3],
            color_range_max: [0.0; 3],
            flip: false,
            flags: 0,
            trc: 0,
            refs: 1,
            prev_frame: false,
        };
        of.data[0] = buf.as_mut_ptr();
        of.linesize[0] = stride as u32;
        out_fn(src, &of);
        let mut l = LOGN.load(Ordering::Relaxed);
        if l < 10 {
            LOGN.store(l + 1, Ordering::Relaxed);
            alog(&format!("output frame idx={} {}x{}", f.frame_index, fw, fh));
        } else {
            l += 1;
            LOGN.store(l + 1, Ordering::Relaxed);
            if l % 50 == 0 {
                alog(&format!(
                    "output frame #{l} idx={} t={}",
                    f.frame_index,
                    f.timestamp
                ));
            }
        }
        (oni.frame_release)(frame);
    }

    (oni.stream_stop)(stream);
    (oni.stream_destroy)(stream);
    (oni.device_close)(dev);
    (oni.shutdown)();
}

fn yuyv_to_bgr(src: &[u8], w: usize, h: usize, stride: usize, dst: &mut [u8]) {
    for y in 0..h {
        let srow = &src[y * w * 2..(y + 1) * w * 2];
        let drow = &mut dst[y * stride..y * stride + w * 3];
        for x in (0..w).step_by(2) {
            let y0 = srow[x * 2] as f32;
            let u = srow[x * 2 + 1] as f32 - 128.0;
            let y1 = srow[x * 2 + 2] as f32;
            let v = srow[x * 2 + 3] as f32 - 128.0;
            for (i, yy) in [y0, y1].iter().enumerate() {
                let (r, g, b) = yuv_pixel(*yy, u, v);
                let px = &mut drow[(x + i) * 3..(x + i) * 3 + 3];
                px[0] = b;
                px[1] = g;
                px[2] = r;
            }
        }
    }
}

fn yuv_pixel(y: f32, u: f32, v: f32) -> (u8, u8, u8) {
    let c = (y - 16.0).max(0.0);
    let r = (1.164 * c + 1.596 * v).clamp(0.0, 255.0);
    let g = (1.164 * c - 0.391 * u - 0.813 * v).clamp(0.0, 255.0);
    let b = (1.164 * c + 2.018 * u).clamp(0.0, 255.0);
    (r as u8, g as u8, b as u8)
}

// ---------------------------------------------------------------- 模块入口

#[no_mangle]
pub extern "C" fn obs_module_load() -> bool {
    unsafe {
        let obs = GetModuleHandleA(b"obs.dll\0".as_ptr() as *const c_char);
        if obs.is_null() {
            return false;
        }
        let reg: Option<
            unsafe extern "C" fn(*const ObsSourceInfo, usize),
        > = sym(obs, CStr::from_bytes_with_nul_unchecked(b"obs_register_source_s\0"))
            .map(|p| std::mem::transmute(p));
        let out = sym(obs, CStr::from_bytes_with_nul_unchecked(b"obs_source_output_video\0"));
        let (reg, out) = match (reg, out) {
            (Some(r), Some(o)) => (r, o),
            _ => return false,
        };
        OUTPUT_VIDEO.store(out as usize, Ordering::SeqCst);

        let mut info = ObsSourceInfo::empty();
        info.id = b"astra_openni_color\0".as_ptr() as *const c_char;
        info.kind = 0; // OBS_SOURCE_TYPE_INPUT
        info.output_flags = OBS_SOURCE_ASYNC_VIDEO;
        info.get_name = Some(get_name);
        info.create = Some(create);
        info.destroy = Some(destroy);
        info.get_width = Some(get_width);
        info.get_height = Some(get_height);

        reg(&info, std::mem::size_of::<ObsSourceInfo>());
        true
    }
}

#[no_mangle]
pub extern "C" fn obs_module_unload() {}

// OBS 33 加载模块时要求存在该导出，缺失会直接判定为「not an OBS plugin」
#[no_mangle]
pub extern "C" fn obs_module_set_pointer(module: *mut c_void) {
    MODULE_PTR.store(module as usize, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn obs_module_name() -> *const c_char {
    b"Astra OpenNI\0".as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn obs_module_description() -> *const c_char {
    b"Orbbec Astra color stream via OpenNI2\0".as_ptr() as *const c_char
}

// libobs API 版本（OBS 33.0.3 = 33*1000000 + 0*1000 + 3）
#[no_mangle]
pub extern "C" fn obs_module_ver() -> u32 {
    33000003
}
