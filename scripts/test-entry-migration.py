#!/usr/bin/env python3
"""Run inside an isolated Linux user/PID/network namespace. No installed kernel is used."""
import argparse
import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlencode
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[1]
PAYLOAD = ROOT / 'patches/qwenpaw-2.3.0.post1-base-path'
PREFIX = '/app/qwenpaw/web'

# Real wheel auth routes, patched auth middleware, base-path middleware and private listener.
# Only the rest of the large application lifespan/business routers are replaced.
CHILD = '''import sys, os
from contextlib import asynccontextmanager
from fastapi import FastAPI, Request
import uvicorn
from qwenpaw.app import fnos_entry, auth
if os.environ.get('FIXTURE_DISABLED_NATIVE') == '1': os.environ['QWENPAW_AUTH_ENABLED']='false'
from types import SimpleNamespace
# The rest of the upstream config package is not part of this minimal kernel fixture.
auth._get_config_cached = lambda: (SimpleNamespace(security=SimpleNamespace(allow_no_auth_hosts=[])), [])
from qwenpaw.app.routers.auth import router
from qwenpaw.app.base_path import BasePathMiddleware
@asynccontextmanager
async def lifespan(app):
    async with fnos_entry.private_listener(app): yield
app=FastAPI(lifespan=lifespan)
app.include_router(router,prefix='/api')
@app.get('/api/version')
async def version(): return {'version':'2.3.0.post1'}
@app.get('/api/fixture')
async def fixture(r:Request): return {'user':r.state.user}
from pathlib import Path
from starlette.staticfiles import StaticFiles
from qwenpaw.app.base_path import ConsoleIndexResponse
console = Path(os.environ['FIXTURE_CONSOLE'])
@app.get('/')
@app.get('/chat')
@app.get('/login')
async def index(): return ConsoleIndexResponse(console/'index.html',base_path='/app/qwenpaw/web')
app.mount('/assets',StaticFiles(directory=console/'assets'))
app.add_middleware(auth.AuthMiddleware)
app.add_middleware(BasePathMiddleware,base_path='/app/qwenpaw/web')
uvicorn.run(app,host='127.0.0.1',port=int(sys.argv[sys.argv.index('--port')+1]),log_level='warning',access_log=False)
'''


def migrate_in_browser(req, gw, old, password, *, recovery=False):
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from threading import Thread
    from playwright.sync_api import expect, sync_playwright

    class Gateway(BaseHTTPRequestHandler):
        def forward(self):
            headers = {k: self.headers[k] for k in ('Host', 'Origin', 'Referer', 'Sec-Fetch-Site', 'Cookie', 'X-CSRF-Token', 'Content-Type') if k in self.headers}
            body = self.rfile.read(int(self.headers.get('Content-Length', 0))) or None
            s, data, response_headers = req(None, self.command, self.path, body, {**headers, **gw})
            self.send_response(s)
            for k, v in response_headers.items():
                if k.lower() in ('content-type', 'set-cookie', 'location', 'cache-control'): self.send_header(k, v)
            self.send_header('Content-Length', str(len(data))); self.end_headers(); self.wfile.write(data)
        do_GET = do_POST = forward
        def log_message(self, *args): pass

    with ThreadingHTTPServer(('127.0.0.1', 0), Gateway) as server, sync_playwright() as pw:
        thread = Thread(target=server.serve_forever, daemon=True); thread.start()
        try:
            webkit = os.environ.get('QWENPAW_TEST_WEBKIT')
            browser = pw.webkit.launch(executable_path=webkit) if webkit else pw.chromium.launch(executable_path=shutil.which('chromium'), args=['--no-sandbox'])
            page = browser.new_page()
            page.add_init_script('delete window.EventSource')
            page.goto(f'http://127.0.0.1:{server.server_port}/app/qwenpaw/#configuration')
            if recovery:
                button = page.get_by_role('button', name='重置 QwenPaw 管理员密码')
                button.click()
                page.get_by_label('新密码', exact=True).fill(password)
                page.get_by_label('确认新密码').fill(password)
                with page.expect_response(lambda r: r.url.endswith('/api/kernel/admin-password') and r.request.method == 'POST') as response:
                    page.get_by_role('button', name='保存新密码').click()
                assert response.value.status == 200
                expect(page.locator('#toast')).to_contain_text('密码已重置')
            elif old:
                page.evaluate("t => localStorage.setItem('qwenpaw_auth_token', t)", old)
                button = page.get_by_role('button', name='迁移旧账号到入口登录'); button.click()
                page.keyboard.press('Escape'); expect(button).to_be_focused()
                button.click()
                with page.expect_response(lambda r: r.url.endswith('/api/entry-login/migrate') and r.request.method=='POST') as response:
                    page.get_by_role('button', name='确认迁移').click()
                assert response.value.status == 200, response.value.status
                expect(page.get_by_role('button', name='已切换为入口登录')).to_be_disabled()
            else:
                button = page.get_by_role('button', name='切换为原生登录'); button.click()
                page.keyboard.press('Escape'); expect(button).to_be_focused()
                for value, expected in [('wrong-fixture',401),(password,200)]:
                    button.click()
                    page.get_by_label('QwenPaw 用户名').fill('owner')
                    page.get_by_label('QwenPaw 密码').fill(value)
                    with page.expect_response(lambda r: r.url.endswith('/api/web-proxy/use-native-login') and r.request.method=='POST') as response:
                        page.get_by_role('button',name='验证并切换').click()
                    assert response.value.status == expected, response.value.status
                    expect(page.locator('#proxyPasswordState')).to_have_text('旧反代密码保护中' if expected==401 else 'QwenPaw 原生账号')
                page.reload(); expect(button).to_have_count(0)
                expect(page.locator('#proxyPasswordState')).to_have_text('QwenPaw 原生账号')
            assert page.evaluate("localStorage.getItem('qwenpaw_auth_token') === null")
            browser.close()
        finally:
            server.shutdown(); thread.join()
    print('PASS: packaged Vue admin password recovery' if recovery else 'PASS: packaged Vue gateway desktop binding' if old else 'PASS: packaged Vue proxy migration: cancel, wrong account refused, confirm, reload and no stored token')


def enter_native_web(kernel, proxy, password):
    from playwright.sync_api import expect, sync_playwright

    with sync_playwright() as pw:
        webkit = os.environ.get('QWENPAW_TEST_WEBKIT')
        browser = (pw.webkit.launch(executable_path=webkit) if webkit else
                   pw.chromium.launch(executable_path=shutil.which('chromium'), args=['--no-sandbox']))
        for origin in (f'http://127.0.0.1:{kernel}', f'http://192.0.2.10:{proxy}'):
            page = browser.new_page()
            assert page.goto(origin + '/').status == 200
            page.locator('input#username').fill('owner')
            page.locator('input#password').fill(password)
            with page.expect_response(lambda r: r.url.endswith('/api/auth/login') and r.request.method == 'POST') as login:
                page.locator('button[type=submit]').click()
            assert login.value.status == 200, login.value.status
            expect(page.locator('textarea:visible,[contenteditable=true]:visible').first).to_be_visible(timeout=20000)
            assert not any('qwenpaw-proxy-' in c['name'] for c in page.context.cookies())
            page.close()
        browser.close()
    print('PASS: native account form reaches Console chat from the direct service root and the password-free reverse proxy without a proxy session')


def enter_web(root, proxy, password):
    import ssl
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from threading import Thread
    from playwright.sync_api import expect, sync_playwright

    class EdgeServer(ThreadingHTTPServer):
        request_queue_size = 128

    class Edge(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'
        def setup(self):
            self.request = tls.wrap_socket(self.request, server_side=True)
            super().setup()
        def forward(self):
            c = http.client.HTTPConnection('127.0.0.1', proxy, timeout=8)
            try:
                body = self.rfile.read(int(self.headers.get('Content-Length', 0))) or None
                c.request(self.command, self.path, body, {k:v for k,v in self.headers.items() if k.lower() not in ('connection','accept-encoding')})
                reply = c.getresponse(); data = reply.read(); self.send_response(reply.status)
                for k,v in reply.getheaders():
                    if k.lower() not in ('connection','transfer-encoding','content-length'): self.send_header(k,v)
                self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
            except (BrokenPipeError, ConnectionResetError): pass
            finally: c.close()
        do_GET = do_POST = forward
        def log_message(self, *args): pass

    key,cert = root/'edge.key', root/'edge.crt'
    subprocess.run(['openssl','req','-x509','-nodes','-newkey','rsa:2048','-days','1','-subj','/CN=localhost','-keyout',str(key),'-out',str(cert)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    with EdgeServer(('127.0.0.1',0),Edge) as server, sync_playwright() as pw:
        tls=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);tls.load_cert_chain(cert,key)
        thread=Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            webkit=os.environ.get('QWENPAW_TEST_WEBKIT')
            browser=(pw.webkit.launch(executable_path=webkit) if webkit else pw.chromium.launch(executable_path=shutil.which('chromium'),args=['--no-sandbox']))
            for url, secure in [(f'http://192.0.2.10:{proxy}', False), (f'https://localhost:{server.server_port}', True)]:
                page=browser.new_page(ignore_https_errors=True,viewport={'width':390,'height':844})
                assert page.goto(url+'/').status == 401
                field=page.get_by_label('访问密码',exact=True);expect(field).to_be_visible()
                assert page.evaluate('window.isSecureContext') == secure
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth && getComputedStyle(document.querySelector("main")).maxWidth === "400px"')
                if secure and (output:=os.environ.get('QWENPAW_LOGIN_SCREENSHOT')): page.screenshot(path=output)
                field.fill(password+'x');page.get_by_role('button',name='登录',exact=True).click()
                expect(page.get_by_role('alert')).to_contain_text('密码不正确'); time.sleep(1.05)
                field.fill(password);page.get_by_role('button',name='登录',exact=True).click()
                expect(page).to_have_title('QwenPaw Console');expect(page.locator('textarea:visible,[contenteditable=true]:visible').first).to_be_visible(timeout=20000)
                assert page.evaluate("async () => (await (await fetch('/app/qwenpaw/web/api/fixture')).json()).user")=='owner'
                sessions=[c for c in page.context.cookies() if 'qwenpaw-proxy-' in c['name']]
                assert len(sessions)==1 and sessions[0]['secure']==secure and sessions[0]['httpOnly'] and sessions[0]['sameSite']=='Strict'
                page.close(); time.sleep(1.05)
            browser.close()
        finally: server.shutdown();thread.join()
    print('PASS: LAN HTTP and TLS reverse proxy both reject wrong passwords and reach Console chat and protected API after login; cookie flags checked in browser')


def run(binary, wheel, panel):
    assert os.geteuid() == 0 and os.readlink('/proc/self/ns/net') != 'net:[4026531840]'
    assert Path('/proc/1/comm').read_text().strip() in ('python3', 'python'), 'requires isolated PID namespace with a reaper'
    if panel:
        # A non-loopback address exercises ordinary HTTP browser rules, not localhost exceptions.
        subprocess.run(['ip','addr','add','192.0.2.10/32','dev','lo'],check=True)
    meta=json.loads((PAYLOAD/'manifest.json').read_text())
    assert hashlib.sha256(wheel.read_bytes()).hexdigest()==meta['official_wheel_sha256']
    with tempfile.TemporaryDirectory(dir=ROOT/'build',prefix='em-') as tmp:
        root=Path(tmp)
        home,var,etc,share,dest=(root/p for p in ('home','var','etc','share','dest'))
        for d in (home,var,etc,share,dest): d.mkdir(mode=0o700)
        lib=home/'venv/lib/python3.11/site-packages'; pkg=lib/'qwenpaw'; bindir=home/'venv/bin'; bindir.mkdir(parents=True)
        with ZipFile(wheel) as z:
            for rel in ('constant.py','app/routers/auth.py','app/rate_limiter.py','security/secret_store.py','plugins/browser_access.py'):
                p=pkg/rel; p.parent.mkdir(parents=True,exist_ok=True); p.write_bytes(z.read('qwenpaw/'+rel))
            metadata, = (name for name in z.namelist() if name.endswith('.dist-info/METADATA'))
            (lib/metadata).parent.mkdir(); (lib/metadata).write_bytes(z.read(metadata))
        for f in ('auth.py','_app.py','fnos_entry.py','base_path.py'):
            shutil.copyfile(PAYLOAD/'payload/app'/f,pkg/'app'/f)
        (home/'venv/.qwenpaw-base-path-patch').write_text('fixture')
        (bindir/'python3').symlink_to(sys.executable)
        (pkg/'__init__.py').touch(); (pkg/'__main__.py').write_text(CHILD)
        (dest/'www').mkdir()
        if panel: shutil.copyfile(panel, dest/'www/index.html')
        else: (dest/'www/index.html').write_text('<html>management fixture</html>')
        key=secrets.token_hex(32); tf=var/'supervisor-token'; tf.write_text(key); tf.chmod(0o600)
        with contextlib.ExitStack() as stack:
            sockets=[stack.enter_context(socket.socket()) for _ in range(3)]
            for s in sockets: s.bind(('127.0.0.1',0))
            kernel,manage,proxy=[s.getsockname()[1] for s in sockets]
        env=dict(os.environ,FIXTURE_CONSOLE=str(PAYLOAD/'payload/console'),TRIM_PKGHOME=str(home),TRIM_PKGVAR=str(var),TRIM_PKGETC=str(etc),TRIM_DATA_SHARE_PATHS=str(share),TRIM_APPDEST=str(dest),TRIM_SERVICE_PORT=str(kernel),SUPERVISOR_PORT=str(manage),QWENPAW_WEB_PROXY_PORT=str(proxy),QWENPAW_WEB_PROXY_PUBLIC='1' if panel else '0',SUPERVISOR_AUTOSTART='1',QWENPAW_GATEWAY_SOCKET=str(var/'app.sock'),QWENPAW_DISABLE_KEYRING='true',QWENPAW_SECRET_DIR=str(root/'fixture-secrets'),PYTHONPATH=str(lib)+os.pathsep+os.environ.get('PYTHONPATH',''),PYTHONDONTWRITEBYTECODE='1')
        def req(port,method,path,body=None,headers=None):
            c=http.client.HTTPConnection('127.0.0.1',port or 1,timeout=4)
            if not port:
                c.sock=socket.socket(socket.AF_UNIX); c.sock.settimeout(4); c.sock.connect(str(var/'app.sock'))
            try:
                c.request(method,path,body=body,headers=headers or {}); r=c.getresponse(); return r.status,r.read(),dict(r.getheaders())
            finally: c.close()
        def admin(path,data):
            return req(manage,'POST',path,json.dumps(data),{'x-supervisor-token':key,'Content-Type':'application/json'})[0]
        with (root/'supervisor.log').open('wb') as log:
            def start():
                return subprocess.Popen([str(binary),'serve'],env=env,cwd=root,stdout=log,stderr=log)
            def ready(p):
                for _ in range(100):
                    try:
                        if req(kernel,'GET',PREFIX+'/api/version')[0]==200: break
                    except OSError: pass
                    assert p.poll() is None, 'supervisor exited before kernel readiness'
                    time.sleep(.1)
                else: raise RuntimeError('fixture kernel did not become ready')
            p=start()
            try:
                ready(p)
                # Exercise the supervisor's /proc rediscovery after its two-second PID cache expires.
                time.sleep(2.1)
                status, raw_status, _ = req(manage,'GET','/status',headers={'x-supervisor-token':key})
                snapshot = json.loads(raw_status)
                assert status == 200 and snapshot['running'] is True, ('fixture not rediscovered', snapshot.get('pid'), req(kernel,'GET',PREFIX+'/api/version')[0])
                record=var/'entry-login-default.v1'
                legacy=var/'proxy-password-default.v1'
                switch='/web-proxy/use-native-login'
                assert admin('/web-proxy/password', {'password':'fixture-value'}) == 404 and not legacy.exists(), 'new installations must not create a shell password'
                assert admin(switch, {'confirm':True,'username':'owner','password':'unused'}) == 409
                assert req(proxy,'GET','/')[0] == 200 and req(proxy,'GET','/api/auth/status')[0] == 200
                assert req(proxy,'GET','/api/fixture')[0] == 401  # Registration is public; business data is not.
                assert admin('/entry-login/migrate',{'token':'invalid-fixture'})==401 and not record.exists()
                reset = '/kernel/admin-password'
                assert admin(reset, {'password':'fixture'}) == 409
                native_password=secrets.token_urlsafe(32)
                s,b,_=req(kernel,'POST',PREFIX+'/api/auth/register',json.dumps({'username':'owner','password':native_password}),{'Content-Type':'application/json'}); assert s==200,s
                old=json.loads(b)['token']; assert admin('/entry-login/migrate',{'token':'invalid-fixture'})==401 and not record.exists()
                old_account=root/'fixture-secrets/auth.json'
                def account_digest(): return hashlib.sha256(old_account.read_bytes()).digest()
                def login(port, value):
                    return req(port, 'POST', PREFIX+'/api/auth/login', json.dumps({'username':'owner','password':value}), {'Content-Type':'application/json'})[:2]
                def verify(port, token):
                    return req(port, 'GET', PREFIX+'/api/auth/verify', headers={'Authorization':'Bearer '+token})[0]
                def check_reset(previous, new, old_token):
                    assert verify(kernel, old_token) == 401
                    assert login(kernel, previous)[0] == 401
                    code, body = login(kernel, new)
                    assert code == 200 and json.loads(body)['username'] == 'owner'
                    assert old_account.stat().st_mode & 0o777 == 0o600
                    assert new.encode() not in old_account.read_bytes()
                    assert json.loads(old_account.read_text())['jwt_secret'].startswith('ENC:')
                    return json.loads(body)['token']
                replacement = secrets.token_urlsafe(24)
                assert admin(reset, {'password':replacement}) == 200 and not record.exists()
                old = check_reset(native_password, replacement, old)
                native_password = replacement
                before=account_digest()
                gw={'x-trim-userid':'1000','x-trim-username':'nas-fixture','x-trim-isadmin':'true'}
                assert req(None,'POST','/app/qwenpaw/api/entry-login/migrate',json.dumps({'token':old}),{**gw,'Content-Type':'application/json'})[0]==403 and not record.exists()
                native={'Authorization':'Bearer '+old}
                def check_native(port):
                    s, status_body, _ = req(port,'GET',PREFIX+'/api/auth/status')
                    assert s == 200, ('native status', 'proxy' if port==proxy else 'kernel', s, p.poll())
                    assert not json.loads(status_body).get('fnos_entry')
                    for path in ('/api/fixture',PREFIX+'/api/fixture'):
                        assert req(port,'GET',path,headers=gw)[0]==401, (port,path)
                        assert req(port,'GET',path,headers=native)[:2]==(200,b'{"user":"owner"}')
                s,_,hh=req(kernel,'GET','/'); assert s in (302,303,307,308) and PREFIX+'/' in str(hh)
                check_native(kernel); check_native(proxy)
                assert req(proxy,'GET','/')[0]==200
                password=secrets.token_hex(8)  # Also covers older, longer shell passwords.
                inputs={'confirm':True,'username':'owner','password':native_password}
                def upgrade_fixture():
                    p.terminate(); assert p.wait(timeout=25)==0
                    salt=secrets.token_bytes(16)
                    legacy.write_text(json.dumps({'salt':list(salt),'hash':list(hashlib.pbkdf2_hmac('sha256',password.encode(),salt,600000))})); legacy.chmod(0o600)
                    worker=start(); ready(worker); return worker
                env['FIXTURE_DISABLED_NATIVE']='1'; p=upgrade_fixture()
                assert not json.loads(req(kernel,'GET',PREFIX+'/api/auth/status')[1])['enabled']
                assert admin(switch,inputs)==401 and legacy.exists() and req(proxy,'GET','/api/fixture')[0]==401
                del env['FIXTURE_DISABLED_NATIVE']; p=upgrade_fixture()
                assert admin('/stop',{})==200
                assert admin(switch,inputs)==503 and legacy.exists()
                assert admin('/start',{})==200; ready(p)
                assert admin('/web-proxy/password',{'password':''})==404 and legacy.exists()
                assert req(proxy,'GET','/')[0]==401
                assert admin(switch,dict(inputs,confirm=False))==400 and legacy.exists()
                assert admin(switch,dict(inputs,username='different-user'))==401 and legacy.exists()
                assert admin(switch,inputs)==200 and not legacy.exists() and not record.exists()
                check_native(proxy)
                p=upgrade_fixture()
                if panel: migrate_in_browser(req, gw, old, None)
                else: assert admin('/entry-login/migrate',{'token':old})==200
                assert json.loads(record.read_text())=={'v':1,'instance':'default','owner':'owner'} and (record.stat().st_mode & 0o777)==0o600
                assert admin('/entry-login/migrate',{'token':old})==409
                check_native(kernel)
                assert req(None,'GET',PREFIX+'/api/fixture',headers={**gw,'x-trim-isadmin':'false'})[0]==403
                assert req(None,'GET',PREFIX+'/api/fixture',headers=gw)[:2]==(200,b'{"user":"owner"}')
                if panel:
                    for route in ('/api/fixture', '/api/models/active', '/api/config/channels'):
                        assert req(proxy, 'GET', route, headers=gw)[0] == 401, route
                    enter_web(root, proxy, password)
                    time.sleep(1.05)  # Independent subsequent login observes the instance throttle.
                assert req(proxy,'GET','/api/fixture',headers={'Authorization':'Bearer '+old})[0]==401
                s,_,h=req(proxy,'POST','/_qwenpaw_proxy/login',urlencode({'password':password}),{'Host':'proxy.test','Origin':'https://proxy.test','Content-Type':'application/x-www-form-urlencoded'}); assert s==303,s
                cookie=next(v for k,v in h.items() if k.lower()=='set-cookie').split(';')[0]; h={'Cookie':cookie}
                assert req(proxy,'GET','/api/fixture',headers=h)[:2]==(200,b'{"user":"owner"}')
                assert json.loads(req(proxy,'GET','/api/auth/status',headers=h)[1])['fnos_entry'] is True
                assert req(proxy,'POST','/api/auth/register','{}',{**h,'Content-Type':'application/json'})[0]==403
                assert admin('/web-proxy/password',{'password':secrets.token_hex(5)})==404 and req(proxy,'GET','/api/fixture',headers=h)[0]==200
                assert admin('/kernel/reset-auth',{})==404 and admin(reset,{})==400 and account_digest()==before
                p.terminate(); assert p.wait(timeout=25)==0, 'fixture supervisor shutdown failed'
                p=start(); ready(p)
                assert req(None,'GET',PREFIX+'/api/fixture',headers=gw)[:2]==(200,b'{"user":"owner"}')
                check_native(kernel)
                assert req(proxy,'GET','/api/fixture',headers=h)[0]==401 and account_digest()==before
                assert req(proxy,'GET','/')[0]==401  # Already configured before restart.
                snapshot=record.read_bytes(); record.write_text(json.dumps({'v':1,'instance':'default','owner':'another'}))
                assert admin(switch,inputs)==409 and legacy.exists()
                assert admin(reset, {'password':'fixture'}) == 409 and account_digest() == before
                record.write_text('broken')
                assert admin(reset, {'password':'fixture'}) == 503 and account_digest() == before
                assert admin(switch,inputs)==503 and legacy.exists()
                record.write_bytes(snapshot)
                assert req(None,'POST','/app/qwenpaw/api'+switch,json.dumps(inputs),{**gw,'Content-Type':'application/json'})[0]==403 and legacy.exists()
                s,_,reply=req(proxy,'POST','/_qwenpaw_proxy/login',urlencode({'password':password}),{'Host':'proxy.test','Origin':'https://proxy.test','Content-Type':'application/x-www-form-urlencoded'}); assert s==303
                h={'Cookie':next(v for k,v in reply.items() if k.lower()=='set-cookie').split(';')[0]}
                assert req(proxy,'GET','/api/fixture',headers=h)[0]==200
                if panel: migrate_in_browser(req,gw,None,native_password)
                else: assert admin(switch,inputs)==200
                assert not legacy.exists() and record.read_bytes()==snapshot
                denied=req(proxy,'POST','/app/qwenpaw/api'+switch,json.dumps(inputs),{**native,'Content-Type':'application/json'})[0]
                assert denied in (401,403,404,405), ('management path on proxy',denied, 'kernel version status',req(kernel,'GET',PREFIX+'/api/version')[0])
                check_native(proxy)
                assert req(proxy,'GET',PREFIX+'/api/fixture',headers=h)[0]==401
                if panel: enter_native_web(kernel, proxy, native_password)
                p.terminate(); assert p.wait(timeout=25)==0
                p=start(); ready(p); check_native(proxy)
                assert req(proxy,'GET',PREFIX+'/api/fixture',headers=h)[0]==401 and account_digest()==before
                assert req(None,'GET',PREFIX+'/api/fixture',headers=gw)[:2]==(200,b'{"user":"owner"}')
                replacement = secrets.token_urlsafe(30)
                assert req(None, 'POST', '/app/qwenpaw/api'+reset, json.dumps({'password':replacement}), {**gw,'Content-Type':'application/json'})[0] == 403
                assert account_digest() == before
                saved_account = old_account.read_bytes()
                old_account.write_bytes(b'{broken')
                assert admin(reset, {'password':replacement}) == 503 and old_account.read_bytes() == b'{broken'
                old_account.write_bytes(saved_account)
                if panel: migrate_in_browser(req, gw, None, replacement, recovery=True)
                else: assert admin(reset, {'password':replacement}) == 200
                token = check_reset(native_password, replacement, old)
                assert verify(proxy, old) == 401 and verify(proxy, token) == 200
                assert record.read_bytes() == snapshot
                assert req(None,'GET',PREFIX+'/api/fixture',headers=gw)[:2]==(200,b'{"user":"owner"}')
                p.terminate(); assert p.wait(timeout=25)==0
                p=start(); ready(p)
                assert verify(proxy, token) == 200 and verify(proxy, old) == 401
                assert login(proxy, replacement)[0] == 200
                if panel: enter_native_web(kernel, proxy, replacement)
                assert replacement.encode() not in (root/'supervisor.log').read_bytes()
                print('PASS: registered native account recovered before/after desktop binding; previous password and sessions rejected; binding, new login and encrypted store persist')
                print('PASS: fresh native default, old write API retired, legacy upgrade retains protection; unbound and desktop-bound switch require native credentials, old session revoked, restart preserves outcome')
            finally:
                p.terminate(); p.wait(timeout=25)

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,required=True);p.add_argument('--wheel',type=Path,required=True)
    p.add_argument('--panel-html', type=Path, help='Exercise migration in Chromium or WebKit with the management HTML from the same FPK')
    a=p.parse_args();run(a.binary.resolve(),a.wheel.resolve(),a.panel_html.resolve() if a.panel_html else None)
