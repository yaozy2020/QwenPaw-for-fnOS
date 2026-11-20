"""Private fnOS HTTP transport for the supervised, single-user Console."""
import asyncio
from contextlib import asynccontextmanager
import json
import os
from pathlib import Path
import socket
import stat
import struct

from starlette.responses import JSONResponse
from starlette.routing import get_route_path

_PRIVATE = object()


def record_path():
    path = os.environ.get('QWENPAW_FNOS_ENTRY_RECORD')
    return Path(path) if path else None


def read_owner():
    path = record_path()
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise ValueError('invalid entry record')
        raw = stream.read(2049)
    if len(raw) > 2048:
        raise ValueError('invalid entry record')
    data = json.loads(raw)
    if data.get('v') != 1 or data.get('instance') != 'default' or not valid_owner(data.get('owner')):
        raise ValueError('invalid entry record')
    return data['owner']


def valid_owner(name):
    return isinstance(name, str) and bool(name.strip()) and len(name.encode()) <= 256 and all(c.isprintable() for c in name)


def migration_owner(token):
    from . import auth
    # A missing legacy store is not proof that a retained installation is new.
    try:
        info = auth.AUTH_FILE.lstat()
    except OSError:
        return None
    if not stat.S_ISREG(info.st_mode) or not auth.is_auth_enabled():
        return None
    account = auth._load_auth_data().get('user')
    if not isinstance(account, dict) or not valid_owner(account.get('username')) or not token:
        return None
    user = auth.verify_token(token)
    return user if user == account['username'] else None


def reset_existing_password(password):
    from . import auth
    import re
    import secrets
    import tempfile

    if not auth.is_auth_enabled():
        return 409
    try:
        fd = os.open(auth.AUTH_FILE, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return 409
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            return 503
        data = json.load(stream)
    if not isinstance(data, dict) or 'user' not in data:
        return 409
    user = data['user']
    if (not isinstance(user, dict) or not valid_owner(user.get('username'))
            or not re.fullmatch(r'[0-9a-f]{64}', user.get('password_hash', ''))
            or not re.fullmatch(r'[0-9a-f]{32}', user.get('password_salt', ''))):
        return 503
    try:
        if read_owner() != user['username']:
            return 412
    except FileNotFoundError:
        pass  # No desktop migration record on a fresh installation.
    signing_key = auth.decrypt_dict_fields(data, auth.AUTH_SECRET_FIELDS).get('jwt_secret')
    if not isinstance(signing_key, str) or not re.fullmatch(r'[0-9a-f]{64}', signing_key):
        return 503
    user['password_hash'], user['password_salt'] = auth._hash_password(password)
    data['jwt_secret'] = secrets.token_hex(32)
    encrypted = auth.encrypt_dict_fields(data, auth.AUTH_SECRET_FIELDS)
    with tempfile.NamedTemporaryFile(dir=auth.AUTH_FILE.parent, mode='w', encoding='utf-8', delete=False) as out:
        try:
            os.fchmod(out.fileno(), 0o600)
            if stat.S_IMODE(os.fstat(out.fileno()).st_mode) != 0o600:
                return 503
            json.dump(encrypted, out, ensure_ascii=False)
            out.flush()
            os.fsync(out.fileno())
            os.replace(out.name, auth.AUTH_FILE)
        finally:
            Path(out.name).unlink(missing_ok=True)
    directory = os.open(auth.AUTH_FILE.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return 204


async def intercept(request, call_next):
    path = get_route_path(request.scope).rstrip('/')
    private = request.scope.get('fnos_entry') is _PRIVATE
    if not private:
        return None
    if request.headers.get('x-fnos-entry-command') == 'admin-password':
        from starlette.responses import Response
        if request.method != 'POST' or path != '/api/auth/fnos-admin-password':
            return Response(status_code=403)
        try:
            password = (await request.json())['password']
            if not isinstance(password, str) or not password.strip():
                return Response(status_code=400)
            status = reset_existing_password(password)
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            status = 503
        return Response(status_code=status)
    if private and request.headers.get('x-fnos-entry-command') == 'prove':
        if request.method != 'POST' or path != '/api/auth/verify':
            return JSONResponse({}, status_code=403)
        owner = migration_owner(request.headers.get('authorization', '').removeprefix('Bearer '))
        if owner is None:
            return JSONResponse({}, status_code=401)
        return JSONResponse({'owner': owner, 'instance': 'default', 'v': 1})
    try:
        owner = read_owner()
    except (OSError, ValueError, TypeError, AttributeError):
        return JSONResponse({}, status_code=503)
    if request.method == 'GET' and path == '/api/auth/status':
        return JSONResponse({'enabled': True, 'has_users': True, 'fnos_entry': True}, headers={'cache-control':'private, no-store'})
    if request.method == 'GET' and path == '/api/auth/verify':
        return JSONResponse({'valid': True, 'username': owner}, headers={'cache-control':'private, no-store'})
    if path == '/api/auth' or path.startswith('/api/auth/'):
        return JSONResponse({'detail': 'Authentication is managed by fnOS'}, status_code=403)
    request.state.user = owner
    return await call_next(request)


@asynccontextmanager
async def private_listener(app):
    if record_path() is None:
        yield
        return
    from uvicorn import Config
    from uvicorn.protocols.http.h11_impl import H11Protocol
    from uvicorn.server import ServerState
    parent = int(os.environ['QWENPAW_FNOS_ENTRY_PARENT'])
    uid = os.geteuid()
    if parent <= 1 or parent != os.getppid():
        raise RuntimeError('invalid fnOS supervisor parent')

    async def private_app(scope, receive, send):
        if os.getppid() != parent:
            await JSONResponse({}, status_code=503)(scope, receive, send)
            return
        native = (b'x-fnos-entry-command', b'native') in scope.get('headers', ())
        await app(dict(scope, fnos_native_proxy=native, fnos_entry=None if native else _PRIVATE), receive, send)

    config = Config(private_app, interface='asgi3', lifespan='off', ws='none', proxy_headers=False, access_log=False, log_config=None)
    config.load()
    state = ServerState()

    class PeerHTTP(H11Protocol):
        def connection_made(self, transport):
            super().connection_made(transport)
            peer = transport.get_extra_info('socket').getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
            pid, peer_uid, _ = struct.unpack('3i', peer)
            self.allowed = (pid, peer_uid) == (parent, uid) and os.getppid() == parent
            if not self.allowed:
                transport.abort()

        def data_received(self, data):
            if self.allowed:
                super().data_received(data)

    server = await asyncio.get_running_loop().create_unix_server(
        lambda: PeerHTTP(config, state, {}), path=b'\0' + f'qwenpaw.entry.v2.{uid}.{os.getpid()}'.encode(), backlog=64,
    )
    try:
        yield
    finally:
        server.close()
        await server.wait_closed()
        for conn in list(state.connections):
            conn.transport.abort()
        for task in list(state.tasks):
            task.cancel()
