# astra-openni — 最小 OBS 插件（Rust）

[![license: GPL-2.0](https://img.shields.io/badge/license-GPL--2.0-blue.svg)](LICENSE)

把 Orbbec Astra 系列（OpenNI 协议，如 PID 0401）的**彩色流**作为 OBS 异步视频源。
这类设备在 Windows 里不是 UVC 摄像头，OBS 原生看不到；本插件补上这一层。

## 目录

```text
obs-astra-openni\
  src\lib.rs              插件全部源码（单文件）
  tools\build.ps1         构建脚本，顺带抹掉产物里的构建机路径
  tools\add_source.py     把源写进 OBS 场景集合 JSON（免手动点选）
  tools\capture_example.py  Python ctypes 直调 OpenNI2 抓帧示例（不走 OBS）
  reference\              obs-studio 33.0.3 头文件（结构体布局依据，GPL-2.0 原样引用）
```

## 实现

- 单个 `cdylib`，无第三方 crate；运行期从已加载的 `obs.dll` 取符号
  （`obs_register_source_s`、`obs_source_output_video`），**不链接 libobs import lib**。
- 结构体布局对齐 obs-studio **33.0.3** 的 `libobs/obs.h`、`obs-source.h`
  （`obs_source_info` 含 `version`/`unversioned_id`，`obs_source_frame` 含 `trc`）。
- 源类型：`OBS_SOURCE_ASYNC_VIDEO`，输出 `VIDEO_FORMAT_BGR3` 640×480@30，水平镜像已纠正。
- OpenNI2 加载顺序：`ASTRA_OPENNI2_PATH` → 插件 DLL 同目录 → 系统搜索路径 → `Program Files\OpenNI2`。

## 构建（Windows / MSVC）

需要 Rust（MSVC target）+ Visual Studio 的 C++ 生成工具（只为拿到 `lib.exe` 需要的库路径，
本插件不链接任何 OBS 库）。在 **x64 Native Tools** 或装了 vcvars 的 shell 里：

```powershell
cargo build --release
```

如果不在 VS 开发环境里，手动把 MSVC 的 lib 目录加进 `LIB`：

```powershell
$vs = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools"
$mvc = (Get-ChildItem "$vs\VC\Tools\MSVC" -Directory | Select-Object -Last 1).FullName
$env:LIB = "$mvc\lib\x64;" + $env:LIB
cargo build --release
```

产物：`target\release\astra_openni.dll`（约 160 KB，无第三方 crate，因此也没有
`cargo vendor` 之类的依赖目录）

**要分发给别人请用脚本构建**：

```powershell
powershell -ExecutionPolicy Bypass -File tools\build.ps1
```

Rust 会把源文件绝对路径、MSVC 会把 PDB 完整路径写进二进制，直接 `cargo build` 得到的 DLL
里带着 `C:\Users\<你的用户名>\...`。脚本编译后会把这串路径原地替换成等长占位符并自检。

## 安装

先从 [Releases](../../releases) 下载 `astra_openni.dll`（或自行编译），再按 OBS 33 的插件布局摆放
（**文件夹名必须与 DLL 名一致**，否则不会被扫描到）：

```text
C:\ProgramData\obs-studio\plugins\astra-openni\
  bin\64bit\astra-openni.dll     <- 把 astra_openni.dll 改名成这个
  bin\64bit\OpenNI2.dll          <- 从 OpenNI SDK 的 samples\bin 复制
  bin\64bit\OpenNI2\Drivers\     <- 含 orbbec.dll
```

重启 OBS 后，添加来源时类型列表里出现 **Astra OpenNI Color**。

### 用脚本免手动添加

关掉 OBS 后：

```powershell
python tools\add_source.py            # 自动挑最近修改过的场景集合
python tools\add_source.py <scene.json>
```

会在原文件旁留一份 `.pre-astra.bak` 备份。

### 不走 OBS 直接抓帧

```powershell
$env:ASTRA_OPENNI2_PATH = "C:\path\to\OpenNI2.dll"
python tools\capture_example.py out.bmp      # 默认第 0 台设备
python tools\capture_example.py out.bmp 1    # 第 1 台
```

运行时诊断日志写在插件目录下 `astra-openni.log`（设备 uri、开流结果、输出帧号）。

## 踩过的坑（改代码前先读）

- OBS 33 加载插件时**必须**能解析到 `obs_module_set_pointer`，缺了只报一句
  `not an OBS plugin`，连模块名都不显示（非 verbose 级别日志里完全没有痕迹）。
- `obs_module_ver` 返回 `uint32_t`（33.0.3 = `33000003`），不是字符串。
- OpenNI 的 `oniGetDeviceList` 拿到的列表**必须在拷走 `uri` 之后**才 `oniReleaseDeviceList`，
  否则 `oniDeviceOpen` 拿到的是已释放内存（初版就是这么崩的）。
- OpenNI2 的驱动目录默认按进程 exe 目录推算，OBS 场景下会算错；
  插件在加载 `OpenNI2.dll` 后会把 `OPENNI2_REDIST` 指向 DLL 所在目录兜底。

## 已知限制

- 只输出彩色流，不含深度/IR（深度是 16bit，OBS 需另做格式与配色映射）。
- 多实例会抢占同一台设备（OpenNI 不支持同一设备被多次打开）。
- 仅实测 Astra Pro（PID 0401）；Pro Plus 的彩色走 UVC，直接用 OBS 的「视频采集设备」即可。

## 前置条件

- Windows x64，OBS Studio 33.x（ABI 按 33.0.3 固定，`obs_module_ver` 返回 `33000003`；
  换 OBS 大版本需重新核对结构体布局）
- Orbbec 深度相机驱动（`ORBBEC Depth Sensor` 在设备管理器里显示正常）
- [Orbbec OpenNI2 SDK](https://www.orbbec.com/developers/openni-sdk/) —— 仓库**不附带**
  其二进制（`OpenNI2.dll` / `orbbec.dll`），请自行下载后按上面「安装」摆放

## 许可证

- 本项目源码：**GPL-2.0**，见 [LICENSE](LICENSE)。选它是因为 `reference/` 下引用了
  obs-studio 的公开头文件（GPL-2.0），OBS 插件生态也基本都用这个许可。
- `reference/*.h` 版权归 OBS Project，原样取自 obs-studio `33.0.3` tag，仅作结构体布局对照，
  不参与编译。
- `OpenNI2.dll`、`orbbec.dll` 版权归 Orbbec，按 Orbbec 的 SDK 许可分发，不在本仓库内。
