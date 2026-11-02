#!/usr/bin/env python3
"""
tools/fpkstudio-pack.py
使用 fpkstudio (https://github.com/shuangji66/fpkstudio) 核心引擎执行纯净 FPK 封装。
核心纪律：
1. 统一物料来源：直接接收 build.sh 严格白名单物化且权限归一化的 staging 目录；
2. 零特权、纯 Unix Socket 管道通信打包；
3. 干净 CI 自动按版本和 SHA256 校验拉取 fpkstudio 引擎二进制，严禁盲信本机工具；
4. 输出由环境变量 FPK_OUT 严格收敛，并返回确切产物路径与签名。
"""

import hashlib
import http.client
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

PROJ_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
BIN_DIR = os.path.join(PROJ_DIR, "tools", "fpkstudio-bin")
FPKSTUDIO_BIN = os.path.join(BIN_DIR, "fpkstudio")
FNPACK_BIN = os.path.join(PROJ_DIR, "tools", "fnpack")

# fpkstudio v1.2.1 官方发布二进制 SHA256 与下载源
EXPECTED_FPKSTUDIO_SHA256 = "68aa3e7cdcf1148f19b1539fc445f82124961a85794373ecc9776c2970267efd"
FPKSTUDIO_DOWNLOAD_URL = "https://github.com/shuangji66/Fndepot/releases/download/2026.10.25/fpkstudio-1.2.1-x86.fpk"

class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, socket_path):
        super().__init__("localhost")
        self.socket_path = socket_path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(self.socket_path)

def ensure_fpkstudio_bin():
    """确保 fpkstudio 可执行文件存在且 SHA256 绝对匹配，CI 环境缺失时自动拉取"""
    if os.path.isfile(FPKSTUDIO_BIN):
        with open(FPKSTUDIO_BIN, "rb") as f:
            current_sha = hashlib.sha256(f.read()).hexdigest()
        if current_sha == EXPECTED_FPKSTUDIO_SHA256:
            return
        print(f"[fpkstudio] 警告: 本地二进制校验和不符 ({current_sha} != {EXPECTED_FPKSTUDIO_SHA256})，重新准备...", file=sys.stderr)

    os.makedirs(BIN_DIR, exist_ok=True)
    print(f"[fpkstudio] 干净环境拉取官方发布包: {FPKSTUDIO_DOWNLOAD_URL}")
    with tempfile.TemporaryDirectory() as td:
        fpk_path = os.path.join(td, "fpkstudio.fpk")
        req = urllib.request.Request(FPKSTUDIO_DOWNLOAD_URL, headers={"User-Agent": "qwenpaw-build/2.0"})
        with urllib.request.urlopen(req, timeout=120) as resp, open(fpk_path, "wb") as f:
            shutil.copyfileobj(resp, f)

        # 解压获取 bin/fpkstudio
        res = subprocess.run(
            ["sh", "-c", f"tar -zxOf '{fpk_path}' app.tgz | tar -zx -C '{td}' bin/fpkstudio"],
            capture_output=True, text=True
        )
        if res.returncode != 0:
            print(f"ERROR: 提取 fpkstudio 失败: {res.stderr}", file=sys.stderr)
            sys.exit(1)

        extracted_bin = os.path.join(td, "bin", "fpkstudio")
        with open(extracted_bin, "rb") as f:
            sha = hashlib.sha256(f.read()).hexdigest()
        if sha != EXPECTED_FPKSTUDIO_SHA256:
            print(f"ERROR: fpkstudio SHA256 校验失败: 期望 {EXPECTED_FPKSTUDIO_SHA256}, 实际 {sha}", file=sys.stderr)
            sys.exit(1)

        shutil.copy2(extracted_bin, FPKSTUDIO_BIN)
        os.chmod(FPKSTUDIO_BIN, 0o755)
        print(f"[fpkstudio] 校验通过，就绪: {FPKSTUDIO_BIN}")

def main():
    if len(sys.argv) < 2:
        print("用法: fpkstudio-pack.py <staging目录>", file=sys.stderr)
        sys.exit(1)

    stage_dir = os.path.abspath(sys.argv[1])
    if not os.path.isdir(stage_dir):
        print(f"ERROR: staging 目录不存在: {stage_dir}", file=sys.stderr)
        sys.exit(1)

    out_dir = os.environ.get("FPK_OUT", os.path.join(PROJ_DIR, "build", "output"))
    os.makedirs(out_dir, exist_ok=True)

    ensure_fpkstudio_bin()

    with tempfile.TemporaryDirectory(prefix="fpkstudio-run-") as tmp_work:
        data_dir = os.path.join(tmp_work, "data")
        socket_path = os.path.join(tmp_work, "fpkstudio.sock")
        prj_root = os.path.join(data_dir, "projects", "qwenpaw")
        os.makedirs(os.path.dirname(prj_root), exist_ok=True)
        os.makedirs(os.path.join(data_dir, "output"), exist_ok=True)

        print("[fpkstudio] 1/3 连接单一 staging 物料源...")
        # 严格将 build.sh 已经规范化物化好的 stage_dir 复制进沙箱
        shutil.copytree(stage_dir, prj_root)

        print("[fpkstudio] 2/3 启动 fpkstudio 守护进程 (Unix Socket)...")
        env = os.environ.copy()
        env["FPKSTUDIO_DATA_DIR"] = data_dir
        env["FPKSTUDIO_SOCKET"] = socket_path
        env["FPKSTUDIO_FNPACK"] = FNPACK_BIN
        env["FPKSTUDIO_LOG_LEVEL"] = "warn"

        proc = subprocess.Popen([FPKSTUDIO_BIN], env=env)
        try:
            for _ in range(50):
                if os.path.exists(socket_path):
                    break
                time.sleep(0.1)

            print("[fpkstudio] 3/3 触发 POST /api/projects/pack...")
            conn = UnixHTTPConnection(socket_path)
            payload = json.dumps({
                "name": "qwenpaw",
                "platforms": [],
                "serverDir": "",
                "copyOut": True
            })
            conn.request("POST", "/api/projects/pack", body=payload, headers={"Content-Type": "application/json"})
            resp = conn.getresponse()
            res_data = json.loads(resp.read().decode("utf-8"))

            if not res_data.get("ok"):
                print(f"ERROR: fpkstudio 打包失败: {res_data}", file=sys.stderr)
                sys.exit(1)

            packed_file = res_data["results"][0]["file"]
            packed_path = res_data["results"][0]["path"]
            final_dst = os.path.join(out_dir, packed_file)

            shutil.copy2(packed_path, final_dst)
            with open(final_dst, "rb") as f:
                sha = hashlib.sha256(f.read()).hexdigest()
            with open(final_dst + ".sha256", "w") as f:
                f.write(f"{sha}  {packed_file}\n")

            # 将准确产出路径写出供后续步骤使用
            print(f"FPK_OUTPUT_PATH={final_dst}")
            print(f"  FPK SHA256: {sha}")
            print(f"  FPK 尺寸: {os.path.getsize(final_dst):,} 字节")

        finally:
            proc.terminate()
            proc.wait()

if __name__ == "__main__":
    main()
