#!/usr/bin/env python3
"""
tools/fpkstudio-pack.py
使用 fpkstudio (https://github.com/shuangji66/fpkstudio) 核心引擎进行高保真纯净 FPK 打包。
特性：
1. 采用临时沙箱工程目录，绝不污染仓库源文件；
2. 注入经过 preflight/audit 校验的白名单运行时文件；
3. 本地启动无特权单文件 fpkstudio 后端实例并监听 unix socket；
4. 调用 POST /api/projects/pack 执行标准打包流水线；
5. 生成的 FPK 自动归档至指定的输出目录。
"""

import http.client
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

PROJ_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
FPKSTUDIO_BIN = os.path.join(PROJ_DIR, "tools", "fpkstudio-bin", "fpkstudio")
FNPACK_BIN = os.path.join(PROJ_DIR, "tools", "fnpack")

class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, socket_path):
        super().__init__("localhost")
        self.socket_path = socket_path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(self.socket_path)

def main():
    out_dir = os.environ.get("FPK_OUT", os.path.join(PROJ_DIR, "build", "output"))
    os.makedirs(out_dir, exist_ok=True)

    if not os.path.isfile(FPKSTUDIO_BIN) or not os.access(FPKSTUDIO_BIN, os.X_OK):
        print(f"ERROR: fpkstudio 可执行文件不存在: {FPKSTUDIO_BIN}", file=sys.stderr)
        sys.exit(1)

    with tempfile.TemporaryDirectory(prefix="fpkstudio-run-") as tmp_work:
        data_dir = os.path.join(tmp_work, "data")
        socket_path = os.path.join(tmp_work, "fpkstudio.sock")
        prj_root = os.path.join(data_dir, "projects", "qwenpaw")
        os.makedirs(prj_root, exist_ok=True)
        os.makedirs(os.path.join(data_dir, "output"), exist_ok=True)

        print("[fpkstudio] 1/4 物化纯净白名单工程结构...")
        # 顶层必需物料
        for item in ["manifest", "LICENSE", "ICON.PNG", "ICON_256.PNG", "cmd", "config", "wizard"]:
            src = os.path.join(PROJ_DIR, item)
            dst = os.path.join(prj_root, item)
            if os.path.isdir(src):
                shutil.copytree(src, dst)
            elif os.path.isfile(src):
                shutil.copy2(src, dst)

        # 运行时 app 目录白名单
        os.makedirs(os.path.join(prj_root, "app", "ui"), exist_ok=True)
        os.makedirs(os.path.join(prj_root, "app", "www"), exist_ok=True)
        os.makedirs(os.path.join(prj_root, "app", "bin"), exist_ok=True)

        shutil.copy2(os.path.join(PROJ_DIR, "app", "ui", "api.cgi"), os.path.join(prj_root, "app", "ui", "api.cgi"))
        shutil.copy2(os.path.join(PROJ_DIR, "app", "ui", "index.cgi"), os.path.join(prj_root, "app", "ui", "index.cgi"))
        shutil.copy2(os.path.join(PROJ_DIR, "app", "ui", "config"), os.path.join(prj_root, "app", "ui", "config"))

        # 前端页面及预压缩产物
        html_src = os.path.join(PROJ_DIR, "web", "dist", "index.html")
        if not os.path.isfile(html_src):
            print(f"ERROR: 前端产物未构建: {html_src}", file=sys.stderr)
            sys.exit(1)
        shutil.copy2(html_src, os.path.join(prj_root, "app", "www", "index.html"))

        # 兄弟预压缩生成
        subprocess.run([
            "node", "-e",
            'const {readFileSync,writeFileSync}=require("fs"),{gzipSync,brotliCompressSync,constants}=require("zlib");'
            'const f=process.argv[1],b=readFileSync(f);'
            'writeFileSync(f+".gz",gzipSync(b,{level:9}));'
            'writeFileSync(f+".br",brotliCompressSync(b,{params:{[constants.BROTLI_PARAM_QUALITY]:11}}))',
            os.path.join(prj_root, "app", "www", "index.html")
        ], check=True)

        # 监督者二进制
        sup_src = os.path.join(PROJ_DIR, "supervisor", "target", "release", "qwenpaw-supervisor")
        if not os.path.isfile(sup_src):
            print(f"ERROR: 监督者二进制缺失: {sup_src}", file=sys.stderr)
            sys.exit(1)
        sup_dst = os.path.join(prj_root, "app", "bin", "qwenpaw-supervisor")
        shutil.copy2(sup_src, sup_dst)
        os.chmod(sup_dst, 0o755)

        print("[fpkstudio] 2/4 启动 fpkstudio 引擎守护进程 (Unix Socket)...")
        env = os.environ.copy()
        env["FPKSTUDIO_DATA_DIR"] = data_dir
        env["FPKSTUDIO_SOCKET"] = socket_path
        env["FPKSTUDIO_FNPACK"] = FNPACK_BIN
        env["FPKSTUDIO_LOG_LEVEL"] = "warn"

        proc = subprocess.Popen([FPKSTUDIO_BIN], env=env)
        try:
            # 等待 socket 就绪
            for _ in range(50):
                if os.path.exists(socket_path):
                    break
                time.sleep(0.1)

            print("[fpkstudio] 3/4 发起 POST /api/projects/pack 构建打包...")
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

            print(f"[fpkstudio] 4/4 打包成功，归档至目标路径: {final_dst}")
            shutil.copy2(packed_path, final_dst)
            # 计算 sha256
            import hashlib
            with open(final_dst, "rb") as f:
                sha = hashlib.sha256(f.read()).hexdigest()
            with open(final_dst + ".sha256", "w") as f:
                f.write(f"{sha}  {packed_file}\n")

            print(f"  FPK SHA256: {sha}")
            print(f"  FPK 尺寸: {os.path.getsize(final_dst):,} 字节")

        finally:
            proc.terminate()
            proc.wait()

if __name__ == "__main__":
    main()
