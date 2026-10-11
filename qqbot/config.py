"""配置加载：从 config.json 读取，环境变量可覆盖任意字段。

支持的环境变量（前缀 QQBOT_）：
  QQBOT_DATA_DIR, QQBOT_WS_URL, QQBOT_WS_TOKEN, QQBOT_API_URL,
  QQBOT_MAGNET_XLSX, QQBOT_MAGNET_RULES, QQBOT_DL_LIMIT,
"""
import json
import os
from pathlib import Path

CONFIG_PATH = Path(__file__).resolve().parent / "config.json"

# 必填字段；缺失会直接报错退出，避免运行到一半 KeyError
_REQUIRED = ("data_dir", "ws_url")
# 环境变量映射：环境变量名 -> config 键
_ENV_MAP = {
    "QQBOT_DATA_DIR": "data_dir",
    "QQBOT_WS_URL": "ws_url",
    "QQBOT_WS_TOKEN": "ws_token",
    "QQBOT_API_URL": "api_url",
    "QQBOT_MAGNET_XLSX": "magnet_xlsx",
    "QQBOT_MAGNET_RULES": "magnet_rules",
    "QQBOT_DL_LIMIT": "dl_limit",
}


def load_config() -> dict:
    with open(CONFIG_PATH, encoding="utf-8") as f:
        cfg = json.load(f)
    # 环境变量覆盖（仅覆盖已设置的键）
    for env_key, cfg_key in _ENV_MAP.items():
        val = os.environ.get(env_key)
        if val:
            cfg[cfg_key] = val
    # 数值字段规范化：dl_limit 转 int
    if cfg.get("dl_limit") is not None:
        try:
            cfg["dl_limit"] = int(cfg["dl_limit"])
        except (TypeError, ValueError):
            cfg["dl_limit"] = None
    # 校验必填
    missing = [k for k in _REQUIRED if not cfg.get(k)]
    if missing:
        raise SystemExit(f"[CONFIG ERROR] missing required field(s): {missing} "
                         f"(check {CONFIG_PATH})")
    return cfg
