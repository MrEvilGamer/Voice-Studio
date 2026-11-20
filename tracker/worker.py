"""Local JSON-line bridge: React owns the UI; Tk owns calibration and overlay."""
import base64
import json
import os
import queue
import sys
import threading
import time
import traceback
import core

COMMANDS = queue.Queue()
OUTPUT_LOCK = threading.Lock()
original_load = core.load_settings

def load_settings():
    settings = original_load()
    settings['mouse'] = False  # Mouse takeover always requires a fresh explicit action.
    settings['learn_clicks'] = False  # Keep calibration learning explicit in this integration.
    return settings

core.load_settings = load_settings
core.ACCENT = '#f97316'
core.DEFAULTS.update(overlay=False, udp=False, bubble_color='#f97316')

def emit(data):
    with OUTPUT_LOCK:
        print(json.dumps(data, allow_nan=False), flush=True)

def read_commands():
    for line in sys.stdin:
        try:
            command = json.loads(line)
            if command.get('action') == 'stop':
                # Prior setting commands persist before task_done(). Stop must
                # not depend on Tk timers or native camera finalizers returning.
                COMMANDS.join()
                os._exit(0)
            COMMANDS.put(command)
        except ValueError:
            pass
    COMMANDS.join()
    os._exit(0)

class IntegratedTracker(core.App):
    def _build(self):
        self.withdraw()
        self.calib_lbl = core.ctk.CTkLabel(self, text='')
        self._update_calib_lbl()

    def __init__(self):
        self.last_emit = 0
        self.message = ''
        self.preview_enabled = False
        self.preview_fps = 0.0
        super().__init__()
        self.title('Voice Studio · Eye Tracking')
        self.withdraw()
        self.after(30, self.poll_commands)
        threading.Thread(target=self.stream_preview, daemon=True).start()

    def poll_commands(self):
        while not COMMANDS.empty():
            command = COMMANDS.get()
            try:
                action = command.get('action')
                if action == 'stop':
                    self.quit_app()
                    return
                if action == 'pause':
                    self.tr.paused = not self.tr.paused
                elif action == 'recenter':
                    self.tr.recenter()
                    self.target = self.render = None
                    self.gaze_f = self._new_gaze_filter()
                elif action == 'calibrate':
                    if not self.tr.face or self.tr.paused:
                        raise ValueError('Start tracking and look straight at the camera first.')
                    self.calibrate()
                elif action == 'accuracy':
                    if self.model.W is None:
                        raise ValueError('Calibrate before testing accuracy.')
                    self.accuracy_test()
                elif action == 'reset':
                    self._reset_calib()
                elif action == 'setting':
                    self.set_setting(command.get('key'), command.get('value'))
                elif action == 'preview':
                    self.preview_enabled = command.get('enabled') is True
                else:
                    raise ValueError('Unknown tracker command.')
                self.message = ''
            except Exception as error:
                self.message = str(error)
            finally:
                COMMANDS.task_done()
        self.after(30, self.poll_commands)

    def quit_app(self):
        self.preview_enabled = False
        self.save()
        # This dedicated worker owns no unsaved state after save(). Native
        # capture/inference finalizers can block interpreter shutdown; exiting
        # the worker releases its camera, sockets and Tk windows together.
        os._exit(0)

    def set_setting(self, key, value):
        ranges = {'camera': (0, 15), 'udp_port': (1, 65535), 'smoothing': (0.1, 5),
                  'stability': (5, 90), 'gaze_smoothing': (0, 1), 'bubble_size': (5, 60),
                  'sens_yaw': (0.1, 3), 'sens_pitch': (0.1, 3), 'sens_roll': (0.1, 3),
                  'sens_pos': (0.1, 3), 'deadzone': (0, 10)}
        toggles = {'mouse', 'overlay', 'udp', 'show_mesh', 'low_light', 'fast_exposure'}
        if key in toggles:
            if not isinstance(value, bool):
                raise ValueError('Expected an on/off setting.')
        elif key in ranges:
            lo, hi = ranges[key]
            if isinstance(value, bool) or not isinstance(value, (float, int)) or not lo <= value <= hi:
                raise ValueError(f'{key} must be between {lo} and {hi}.')
            if key in {'camera', 'udp_port'}:
                value = int(value)
        elif key == 'resolution':
            if value not in {'640x480', '1280x720', '1920x1080', '2560x1440'}:
                raise ValueError('Unsupported camera resolution.')
        else:
            raise ValueError('Unknown tracker setting.')
        self.s[key] = value
        if key in {'camera', 'resolution', 'fast_exposure'}:
            self._restart_cam()
        if key == 'smoothing':
            self.tr.apply_smoothing()
        if key == 'stability':
            self.gaze_f = self._new_gaze_filter()
        self.save()

    def stream_preview(self):
        """One latest-frame slot, no inference wait and no accumulated frame queue."""
        previous = None
        last_sent = 0.0
        count = 0
        interval_start = time.perf_counter()
        while self.tr.running:
            sample = self.tr.preview_sample
            now = time.perf_counter()
            if not self.preview_enabled or sample is None or sample is previous or now - last_sent < 1 / 30:
                time.sleep(0.003)
                continue
            previous = sample
            captured, image = sample
            height, width = image.shape[:2]
            image = core.cv2.resize(image, (640, max(1, int(height * 640 / width))), interpolation=core.cv2.INTER_LINEAR)
            image = core.cv2.flip(image, 1)
            landmarks = self.tr.preview_landmarks
            if self.s['show_mesh'] and self.tr.face and not self.tr.paused and landmarks is not None:
                points = (landmarks * [image.shape[1], image.shape[0]]).astype(int)
                for a, b in core.CONTOURS:
                    core.cv2.line(image, tuple(points[a]), tuple(points[b]), (217, 184, 0), 1, core.cv2.LINE_AA)
                for iris in core.IRIS:
                    centre = points[list(iris)].mean(0).astype(int)
                    core.cv2.circle(image, tuple(centre), 3, (80, 80, 255), 1, core.cv2.LINE_AA)
            ok, encoded = core.cv2.imencode('.jpg', image, [core.cv2.IMWRITE_JPEG_QUALITY, 70])
            if ok:
                emit({'type': 'preview', 'image': 'data:image/jpeg;base64,' + base64.b64encode(encoded).decode('ascii'),
                      'ageMs': round((time.perf_counter() - captured) * 1000, 1), 'sentAt': round(time.time() * 1000)})
                last_sent = now
                count += 1
                if last_sent - interval_start >= 1:
                    self.preview_fps = count / (last_sent - interval_start)
                    count = 0
                    interval_start = last_sent

    def _tick_ui(self):
        now = time.perf_counter()
        if now - self.last_emit < 0.2:
            return
        self.last_emit = now
        tr = self.tr
        emit({'running': True, 'paused': tr.paused, 'face': tr.face,
              'status': tr.status, 'error': self.message, 'fps': round(tr.fps, 1),
              'cameraFps': round(tr.cam_fps, 1), 'latency': round(tr.latency * 1000, 1),
              'confidence': round(tr.conf * 100), 'cameraInfo': tr.cam_info,
              'calibrated': self.model.W is not None, 'calibrating': self.calibrating,
              'calibration': self.calib_lbl.cget('text'), 'previewFps': round(self.preview_fps, 1),
              'pose': (tr.pose - tr.center).tolist() if tr.pose is not None else None,
              'settings': self.s})

if __name__ == '__main__':
    if '--self-test' in sys.argv:
        model = core.GazeModel()
        emit({'ok': True, 'mediapipe': core.mp.__version__, 'calibrated': model.W is not None})
    else:
        threading.Thread(target=read_commands, daemon=True).start()
        try:
            IntegratedTracker().mainloop()
        except Exception as error:
            emit({'running': False, 'error': str(error)})
            traceback.print_exc(file=sys.stderr)
            sys.exit(1)
