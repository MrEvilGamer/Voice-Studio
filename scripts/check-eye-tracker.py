"""Smoke-test the packaged worker using isolated settings and no pointer/game output."""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import statistics
import threading
import time

root = Path(__file__).resolve().parents[1]
worker = root / 'src-tauri/tracker-runtime/eye-worker/eye-worker.exe'
launch = [sys.executable, '-u', str(root / 'tracker/worker.py')] if '--source' in sys.argv else [str(worker)]
with tempfile.TemporaryDirectory(prefix='voice-studio-eye-check-') as data:
    env = {**os.environ, 'VOICE_STUDIO_TRACKER_DATA': data}
    if '--profile-current' in sys.argv:
        settings_file = Path(os.environ['APPDATA']) / 'Voice Studio/Eye Tracker/settings.json'
        settings = json.loads(settings_file.read_text())
        settings.update(mouse=False, udp=False, overlay=False)
        Path(data, 'settings.json').write_text(json.dumps(settings))
    check = subprocess.run([*launch, '--self-test'], env=env, capture_output=True, text=True, timeout=30)
    assert check.returncode == 0, check.stderr
    assert json.loads(check.stdout)['ok']
    print('Runtime self-test passed.', flush=True)
    with open(Path(data) / 'worker.log', 'w') as log:
        process = subprocess.Popen(launch, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=log, text=True, env=env, creationflags=0x08000000)
        snapshots = queue.Queue()
        previews = []
        def reader():
            for line in process.stdout:
                try:
                    event = json.loads(line)
                    if event.get('type') == 'preview': previews.append((time.perf_counter(), event['ageMs']))
                    else: snapshots.put(event)
                except ValueError: pass
        threading.Thread(target=reader, daemon=True).start()
        def wait_for(predicate, timeout=25):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                assert process.poll() is None, Path(data, 'worker.log').read_text()
                try: state = snapshots.get(timeout=1)
                except queue.Empty: continue
                if predicate(state): return state
            raise AssertionError('Timed out waiting for tracker snapshot: ' + Path(data, 'worker.log').read_text())
        def command(action, **values):
            process.stdin.write(json.dumps({'action': action, **values}) + '\n')
            process.stdin.flush()
        try:
            state = wait_for(lambda s: bool(s.get('settings')))
            assert not state['settings']['mouse'] and not state['settings']['udp'] and not state['settings']['overlay']
            print('Startup and safe output defaults passed.', flush=True)
            command('setting', key='stability', value=55)
            wait_for(lambda s: s.get('settings', {}).get('stability') == 55)
            command('pause')
            wait_for(lambda s: s.get('paused') is True)
            command('pause')
            wait_for(lambda s: s.get('paused') is False)
            command('setting', key='udp_port', value=70000)
            wait_for(lambda s: 'between' in s.get('error', ''))
            command('recenter')
            wait_for(lambda s: not s.get('error'))
            print('Settings, pause/resume, validation and recenter passed.')
            command('preview', enabled=True)
            state = wait_for(lambda s: s.get('previewFps', 0) > 0 or 'No camera' in s.get('status', ''), timeout=15)
            if previews:
                time.sleep(8 if '--profile-current' in sys.argv else 3)
                frames = previews[5:]
                while not snapshots.empty(): state = snapshots.get_nowait()
                if len(frames) > 1:
                    fps = (len(frames)-1)/(frames[-1][0]-frames[0][0])
                    age = statistics.median(frame[1] for frame in frames)
                    print(f'Preview measured: {fps:.1f} fps; median capture-to-IPC age {age:.1f} ms; frames {len(frames)}.', flush=True)
                    print(f'Camera: {state.get("cameraFps")} fps; tracking: {state.get("fps")} fps.', flush=True)
                    assert age < 400, 'Preview is publishing stale camera frames.'
                    camera_fps = state.get('cameraFps', 0)
                    if camera_fps >= 10:
                        assert fps >= min(20, camera_fps * 0.65), 'Preview is throttled below the available camera frame rate.'
                command('preview', enabled=False)
                time.sleep(0.3)
                stopped_count = len(previews)
                time.sleep(0.5)
                assert len(previews) == stopped_count, 'Preview did not stop when hidden.'
            print('Camera:', state.get('status'), '| preview available:', bool(previews), flush=True)
            command('stop')
            assert process.wait(timeout=8) == 0
            saved = json.loads(Path(data, 'settings.json').read_text())
            assert saved['stability'] == 55
            print('Clean shutdown and persistence passed.')
        except Exception:
            failure = Path(data, 'worker.log').read_text()
            (root / 'tracker/check-failure.log').write_text(failure, encoding='utf-8')
            print(failure[-5000:], flush=True)
            raise
        finally:
            if process.poll() is None: process.kill(); process.wait()
