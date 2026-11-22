"""把「Astra OpenNI Color」源直接写进 OBS 的场景集合 JSON，省去手动点选。

用法：
    python add_source.py              # 自动挑最近修改过的场景集合
    python add_source.py <scene.json> # 指定场景集合文件

OBS 必须处于关闭状态，否则退出时会把内存里的场景覆盖回磁盘。
脚本会在原文件旁边留一份 <原名>.pre-astra.bak。
"""
import json
import os
import shutil
import sys
import uuid
from pathlib import Path
from typing import Optional

NAME = "Astra OpenNI Color"
SID = "astra_openni_color"


def scenes_dir() -> Path:
    appdata = os.environ.get("APPDATA")
    if not appdata:
        sys.exit("APPDATA not set; run this on Windows")
    return Path(appdata) / "obs-studio" / "basic" / "scenes"


def pick_scene(arg: Optional[str]) -> Path:
    if arg:
        p = Path(arg).expanduser()
        if not p.is_file():
            sys.exit(f"scene file not found: {p}")
        return p
    d = scenes_dir()
    if not d.is_dir():
        sys.exit(f"OBS scenes directory not found: {d}")
    files = sorted(d.glob("*.json"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not files:
        sys.exit(f"no scene collection (*.json) in {d}")
    return files[0]


def main() -> None:
    scene_file = pick_scene(sys.argv[1] if len(sys.argv) > 1 else None)
    print(f"scene collection: {scene_file}")

    shutil.copyfile(scene_file, str(scene_file) + ".pre-astra.bak")
    with open(scene_file, encoding="utf-8") as f:
        d = json.load(f)

    if any(s.get("name") == NAME for s in d.get("sources", [])):
        print(f"{NAME} already present, nothing to do")
        return

    suid = str(uuid.uuid4())
    d.setdefault("sources", []).append({
        "prev_ver": 503316485,
        "name": NAME,
        "uuid": suid,
        "id": SID,
        "versioned_id": SID,
        "settings": {},
        "mixers": 0,
        "sync": 0,
        "flags": 0,
        "volume": 1.0,
        "balance": 0.5,
        "enabled": True,
        "muted": False,
        "push-to-mute": False,
        "push-to-mute-delay": 0,
        "push-to-talk": False,
        "push-to-talk-delay": 0,
        "hotkeys": {},
        "deinterlace_mode": 0,
        "deinterlace_field_order": 0,
        "monitoring_type": 0,
        "private_settings": {},
    })

    scene = next((s for s in d["sources"] if s.get("id") == "scene"), None)
    if scene is None:
        sys.exit("no scene source in this collection; create a scene in OBS first")

    items = scene["settings"].setdefault("items", [])
    items.append({
        "name": NAME,
        "source_uuid": suid,
        "visible": True,
        "locked": False,
        "rot": 0.0,
        "pos": {"x": 0.0, "y": 0.0},
        "scale": {"x": 1.0, "y": 1.0},
        "align": 5,
        "bounds_type": 0,
        "bounds_align": 0,
        "bounds": {"x": 0.0, "y": 0.0},
        "crop_left": 0,
        "crop_top": 0,
        "crop_right": 0,
        "crop_bottom": 0,
        "id": 1,
        "group_item_backup": False,
        "scale_filter": "disable",
        "blend_method": "default",
        "blend_type": "normal",
        "show_transition": {"duration": 0},
        "hide_transition": {"duration": 0},
        "private_settings": {},
    })
    scene["settings"]["id_counter"] = len(items) + 1

    with open(scene_file, "w", encoding="utf-8") as f:
        json.dump(d, f, ensure_ascii=False, indent=4)
    print(f"added {NAME} ({suid})")


if __name__ == "__main__":
    main()
