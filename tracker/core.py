"""
OpenGaze - free webcam head + eye tracker with a full desktop UI.

Global hotkeys (work in games):  F8 recenter   F9 pause tracking   F10 mouse control
"""
import ctypes, ctypes.wintypes, json, math, os, socket, struct, threading, time
import tkinter as tk

import cv2
import numpy as np
import mediapipe as mp
import customtkinter as ctk
from PIL import Image, ImageTk

try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    pass

HERE = os.environ.get("VOICE_STUDIO_TRACKER_DATA", os.path.dirname(os.path.abspath(__file__)))
os.makedirs(HERE, exist_ok=True)
CALIB_FILE = os.path.join(HERE, "calibration.json")
SETTINGS_FILE = os.path.join(HERE, "settings.json")
FACE_MODEL_FILE = os.path.join(HERE, "face_model.npy")

DEFAULTS = {
    "camera": 0, "resolution": "1920x1080", "smoothing": 1.0,
    "mouse": False, "overlay": True, "udp": True, "udp_port": 4242,
    "bubble_size": 28, "bubble_color": "#00e5ff", "gaze_smoothing": 0.3,
    "head_mouse_yaw": 20.0, "head_mouse_pitch": 12.0,
    "sens_yaw": 1.0, "sens_pitch": 1.0, "sens_roll": 1.0, "sens_pos": 1.0,
    "deadzone": 0.5, "show_mesh": True, "learn_clicks": True, "stability": 45.0, "low_light": True, "fast_exposure": False, "exposure": -7.0,
}

ACCENT = "#00b8d9"
BG, PANEL, CARD, TEXT, MUTED = "#0e1116", "#151a21", "#1c232c", "#e6edf3", "#8b98a5"


def load_settings():
    s = dict(DEFAULTS)
    try:
        s.update(json.load(open(SETTINGS_FILE)))
    except Exception:
        pass
    if s.pop("exposure", None) is not None:      # settings from the manual-exposure build
        s["fast_exposure"] = False
    return s


def list_cameras():
    try:
        from pygrabber.dshow_graph import FilterGraph
        names = FilterGraph().get_input_devices()
        if names:
            return names
    except Exception:
        pass
    return [f"Camera {i}" for i in range(3)]


# ---------------------------------------------------------------- filtering
class OneEuro:
    def __init__(self, min_cutoff=1.0, beta=0.02, d_cutoff=1.0):
        self.min_cutoff, self.beta, self.d_cutoff = min_cutoff, beta, d_cutoff
        self.x = self.dx = self.t = None

    @staticmethod
    def _a(cutoff, dt):
        r = 2 * math.pi * cutoff * dt
        return r / (r + 1)

    def __call__(self, x, t):
        x = np.asarray(x, dtype=float)
        if self.x is None:
            self.x, self.dx, self.t = x, np.zeros_like(x), t
            return x
        dt = max(t - self.t, 1e-3)
        self.t = t
        self.dx = self.dx + self._a(self.d_cutoff, dt) * ((x - self.x) / dt - self.dx)
        cutoff = self.min_cutoff + self.beta * np.abs(self.dx)
        self.x = self.x + self._a(cutoff, dt) * (x - self.x)
        return self.x


class FixationFilter:
    """Fixation/saccade filter fed with *unfiltered* gaze, so there is no built-in lag.

    * Fixating: every sample is averaged into the fixation (noise drops ~1/sqrt(N)),
      so the pointer sits still. The lock radius adapts to your measured noise.
    * Big saccade (> 3x radius, confident frame): jump on the very first frame.
    * Small move (radius..3x): needs 2 consecutive samples that agree -> one noisy
      frame can never move the pointer.
    * After a jump the average restarts from the new sample, so it lands in one step
      instead of creeping there in stages."""

    def __init__(self, radius=45.0):
        self.base = radius
        self.THR = 1.3
        self.sigma = radius / 3
        self.fix = None
        self.n = 0
        self.pending = []
        self.recent = []

    @property
    def radius(self):
        return max(self.base, 2.5 * self.sigma)

    def _jump(self, p):
        self.fix, self.n, self.pending = np.asarray(p, float), 1, []
        self.recent = []

    def __call__(self, p, conf=1.0):
        p = np.asarray(p, float)
        if self.fix is None:
            self._jump(p)
            return self.fix
        if conf < 0.35:                               # untrustworthy frame: hold still
            return self.fix
        R = self.radius
        d = np.linalg.norm(p - self.fix)
        if d < R:
            self.pending = []
            self.recent = (self.recent + [p])[-5:]
            # small shift: the last 5 samples agree on a spot the fixation isn't at -> move now
            # (a 5-sample mean has ~0.45x the noise, so 1.1*sigma is a >2-std-of-mean shift)
            if len(self.recent) == 5:
                m = np.mean(self.recent, 0)
                if np.linalg.norm(m - self.fix) > self.THR * self.sigma:
                    self._jump(m)
                    self.n = 5
                    return self.fix
            self.sigma = 0.97 * self.sigma + 0.03 * min(d, 3 * self.sigma) * 1.25
            self.n = min(self.n + 1, 30)
            self.fix = self.fix + (p - self.fix) / self.n
        else:
            self.pending.append(p)
            if len(self.pending) >= 2:
                a, b_ = self.pending[-2], self.pending[-1]
                if np.linalg.norm(b_ - a) < R:        # two frames agree on the new spot
                    self._jump((a + b_) / 2)
                elif len(self.pending) >= 3:
                    self._jump(b_)
        return self.fix


# ---------------------------------------------------------------- tracker
# Rigid (non-deforming) landmarks: forehead, nose, cheekbones, eye corners. No mouth/jaw,
# so talking or smiling doesn't move the head estimate.
STABLE = [10, 151, 9, 8, 168, 6, 197, 195, 5, 4, 1, 19, 94, 2, 33, 133, 362, 263,
          127, 356, 234, 454, 93, 323, 116, 345, 50, 280, 36, 266, 142, 371, 64, 294,
          98, 327, 115, 344, 102, 331, 48, 278, 129, 358, 49, 279, 126, 355, 71, 301,
          63, 293, 105, 334, 107, 336, 55, 285, 193, 417, 245, 465, 188, 412]
IRIS = [(468, 469, 470, 471, 472), (473, 474, 475, 476, 477)]
EYE_RING = [[33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246],
            [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466]]
EYE_KEY = [(33, 133, 159, 145), (263, 362, 386, 374)]  # outer, inner, upper lid, lower lid
IPD_MM = 63.0
CONTOURS = mp.solutions.face_mesh.FACEMESH_CONTOURS
N_FEAT = 11


class Tracker(threading.Thread):
    """Features: [lx, ly, rx, ry, open, yaw, pitch, roll, hx, hy, hz]"""

    def __init__(self, s):
        super().__init__(daemon=True)
        self.s = s
        self.running = True
        self.paused = False
        self.restart = True
        self.status = "Starting camera..."
        self.frame = None
        self.preview_sample = None  # Latest raw camera frame; independent of inference.
        self.preview_landmarks = None
        self.features = None
        self.history = []           # (t, features) for click learning
        self.blink = False
        self.pose = None
        self.out_pose = np.zeros(6)
        self.center = np.zeros(6)
        self.fps = 0.0
        self.face = False
        self.model3d = None          # personalised 3D face (mm), built once from your own face
        try:
            self.model3d = np.load(FACE_MODEL_FILE)
        except Exception:
            pass
        self.rvec = self.tvec = None
        self.open_ref = None
        self.head_f = OneEuro(1.0, 0.05)
        self.eye_f = OneEuro(1.5, 0.5)
        self.conf = 0.0
        self.latency = 0.0
        self.cam_fps = 0.0
        self._cam_t = time.perf_counter()
        self._attempt = 0
        self._cam_frames = 0
        self._opened_at = 0.0
        self.cam_info = ""
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.mesh = mp.solutions.face_mesh.FaceMesh(
            max_num_faces=1, refine_landmarks=True,
            min_detection_confidence=0.6, min_tracking_confidence=0.6)
        self.cap = None

    def apply_smoothing(self):
        c = 1.0 / max(self.s["smoothing"], 0.1)
        self.head_f.min_cutoff = c
        self.eye_f.min_cutoff = 1.5 * c

    def recenter(self):
        if self.pose is not None:
            self.center = self.pose.copy()

    def _open(self):
        if self.cap:
            self.cap.release()
        backend = [cv2.CAP_DSHOW, cv2.CAP_MSMF][self._attempt % 2]
        self.cap = cv2.VideoCapture(self.s["camera"], backend)
        w, h = map(int, self.s["resolution"].split("x"))
        c = self.cap
        c.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))   # MJPG = high fps over USB
        c.set(cv2.CAP_PROP_FRAME_WIDTH, w)
        c.set(cv2.CAP_PROP_FRAME_HEIGHT, h)
        c.set(cv2.CAP_PROP_FPS, 120)                                  # ask for the max, driver clamps
        c.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))
        c.set(cv2.CAP_PROP_BUFFERSIZE, 1)
        if self.s["fast_exposure"]:
            # Auto-exposure in a dim room stretches each frame (-> 10-15 fps). Pin a short exposure
            # and let gain + the low-light boost make up the brightness.
            c.set(cv2.CAP_PROP_AUTO_EXPOSURE, 0.25 if backend == cv2.CAP_DSHOW else 1)
            self._exp = -5.0
            c.set(cv2.CAP_PROP_EXPOSURE, self._exp)
            c.set(cv2.CAP_PROP_GAIN, 255)
        else:
            c.set(cv2.CAP_PROP_AUTO_EXPOSURE, 0.75 if backend == cv2.CAP_DSHOW else 3)
        self.cam_info = f"{int(c.get(cv2.CAP_PROP_FRAME_WIDTH))}x{int(c.get(cv2.CAP_PROP_FRAME_HEIGHT))} " \
                        f"{['DSHOW', 'MSMF'][self._attempt % 2]}"
        self._opened_at = time.perf_counter()
        self._cam_frames = 0
        self.restart = False

    def _grab(self):
        """Dedicated capture thread: always hands the tracker the newest frame (no queue lag)."""
        while self.running:
            if self.restart:
                self._open()
            ok, frame = self.cap.read() if self.cap.isOpened() else (False, None)
            if not ok:
                self.preview_sample = None
                self._latest = None
                self.cam_ok = False
                time.sleep(0.5)
                self.restart = True
                continue
            self.cam_ok = True
            now = time.perf_counter()
            self.preview_sample = (now, frame)
            self.cam_fps = 0.9 * self.cam_fps + 0.1 / max(now - self._cam_t, 1e-3)
            self._cam_t = now
            self._cam_frames += 1
            # after 3 s, if the camera is slow, try the other capture backend once
            if self._cam_frames == 90 or (now - self._opened_at > 3 and self._cam_frames < 60 and self._attempt < 1):
                if self.cam_fps < 20 and self._attempt < 1:
                    self._attempt += 1
                    self.restart = True
                    continue
            with self._cv:
                self._latest = (time.perf_counter(), frame)
                self._cv.notify()
        if self.cap:
            self.cap.release()  # Only the capture thread owns camera teardown.

    def run(self):
        self._cv = threading.Condition()
        self._latest = None
        self.cam_ok = True
        threading.Thread(target=self._grab, daemon=True).start()
        clahe = cv2.createCLAHE(clipLimit=2.0, tileGridSize=(4, 4))
        roi = None
        last = time.perf_counter()
        while self.running:
            with self._cv:
                self._cv.wait(0.5)
                item, self._latest = self._latest, None
            if item is None:
                if not self.cam_ok:
                    self.status = "No camera found - check the camera list or Windows camera privacy settings"
                    self.face = False
                    self.features = None
                continue
            t_cap, frame = item
            frame = cv2.flip(frame, 1)
            h, w = frame.shape[:2]
            frame = self._auto_bright(frame, roi)
            now = time.perf_counter()
            self.fps = 0.9 * self.fps + 0.1 / max(now - last, 1e-3)
            last = now
            if self.paused:
                self.status = "Paused (F9)"
                self.features = None
                self.frame = self._small(cv2.cvtColor(frame, cv2.COLOR_BGR2RGB))
                continue

            # Work on a native-resolution crop around the face: faster, and the contrast
            # boost helps a lot in dim rooms or with a camera off to the side.
            x0, y0, x1, y1 = roi if roi else (0, 0, w, h)
            crop = frame[y0:y1, x0:x1]
            if self.s["low_light"]:
                lab = cv2.cvtColor(crop, cv2.COLOR_BGR2LAB)
                lab[..., 0] = clahe.apply(lab[..., 0])
                crop = cv2.cvtColor(lab, cv2.COLOR_LAB2BGR)
            rgbc = cv2.cvtColor(crop, cv2.COLOR_BGR2RGB)
            res = self.mesh.process(rgbc)
            rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
            if res.multi_face_landmarks:
                cw, ch = x1 - x0, y1 - y0
                lm = res.multi_face_landmarks[0].landmark
                pts = np.array([(q.x * cw + x0, q.y * ch + y0, q.z * cw) for q in lm])
                self.preview_landmarks = pts[:, :2] / np.array([w, h])
                self.status = "Tracking" + ("  (blink)" if self.blink else "")
                self._process(pts, w, h, now, rgb)
                self.face = True
                bx0, by0 = pts[:, :2].min(0)
                bx1, by1 = pts[:, :2].max(0)
                pad = 0.45 * max(bx1 - bx0, by1 - by0)
                roi = (int(max(bx0 - pad, 0)), int(max(by0 - pad, 0)),
                       int(min(bx1 + pad, w)), int(min(by1 + pad, h)))
                self.latency = time.perf_counter() - t_cap
            else:
                if roi:                      # lost it in the crop: retry on the full frame
                    roi = None
                    self.mesh.reset() if hasattr(self.mesh, "reset") else None
                    continue
                self.face = False
                self.preview_landmarks = None
                self.features = None
                self.conf = 0.0
                self.rvec = None
                self.status = ("Too dark to see your face - turn on a light in front of you"
                               if getattr(self, "brightness", 255) < 35 else
                               "No face detected - face the camera, improve lighting")
            self.frame = self._small(rgb)
    def _auto_bright(self, frame, roi):
        """Keep the face well exposed without giving up frame rate:
        1) digital gain (free), 2) only if still too dark, lengthen exposure one step
        (never beyond ~1/30 s so the camera keeps 30 fps), shorten it when bright."""
        x0, y0, x1, y1 = roi if roi else (0, 0, frame.shape[1], frame.shape[0])
        m = float(cv2.cvtColor(frame[y0:y1:4, x0:x1:4], cv2.COLOR_BGR2GRAY).mean()) + 1e-3
        self.brightness = m
        self._bright_n = getattr(self, "_bright_n", 0) + 1
        if self.s["fast_exposure"] and self._bright_n % 20 == 0 and self.cap is not None:
            exp = getattr(self, "_exp", -5.0)
            if m < 45 and exp < -5:
                exp += 1
            elif m > 150 and exp > -9:
                exp -= 1
            if exp != getattr(self, "_exp", None):
                self._exp = exp
                self.cap.set(cv2.CAP_PROP_EXPOSURE, exp)
        if self.s["low_light"] and m < 70:
            g = min(100.0 / m, 3.0)
            lut = np.clip(((np.arange(256) / 255.0) ** 0.75) * 255 * g ** 0.8, 0, 255).astype(np.uint8)
            frame = cv2.LUT(frame, lut)
        return frame

    @staticmethod
    def _small(img):
        h, w = img.shape[:2]
        if w > 720:
            img = cv2.resize(img, (720, h * 720 // w), interpolation=cv2.INTER_AREA)
        return img

    def _build_model(self, pts):
        """Personal 3D face model: landmark cloud scaled to real mm via IPD."""
        P = pts[STABLE].copy()
        ipd = np.linalg.norm(pts[468, :2] - pts[473, :2])
        P = (P - pts[168]) * (IPD_MM / ipd)
        P[:, 1] *= -1           # y up
        P[:, 2] *= -1           # z towards camera
        self.model3d = P
        self.rvec = self.tvec = None
        np.save(FACE_MODEL_FILE, P)

    def _process(self, pts, w, h, now, img):
        # ---- head pose (PnP on ~66 landmarks of your own face model)
        yaw_raw = abs(pts[1, 0] - (pts[33, 0] + pts[263, 0]) / 2) / (abs(pts[263, 0] - pts[33, 0]) + 1e-6)
        if self.model3d is None:
            if yaw_raw > 0.04:
                self.status = "Look straight at the camera to build your face model"
                return
            self._build_model(pts)
        cam = np.array([[w, 0, w / 2], [0, w, h / 2], [0, 0, 1]], dtype=np.float64)
        img2d = pts[STABLE, :2].astype(np.float64)
        if self.rvec is None:
            ok, rvec, tvec = cv2.solvePnP(self.model3d, img2d, cam, None, flags=cv2.SOLVEPNP_SQPNP)
        else:
            ok, rvec, tvec = cv2.solvePnP(self.model3d, img2d, cam, None, self.rvec.copy(),
                                          self.tvec.copy(), True, cv2.SOLVEPNP_ITERATIVE)
        if not ok:
            self.rvec = None
            return
        self.rvec, self.tvec = rvec, tvec
        R, _ = cv2.Rodrigues(rvec)
        # model axes: x right, y up, z towards camera; camera: y down, z forward
        R = R @ np.diag([1, -1, -1])
        pitch, yaw, roll = cv2.RQDecomp3x3(R)[0]
        pitch = (pitch + 180) % 360 - 180
        pitch = pitch - 180 if pitch > 90 else (pitch + 180 if pitch < -90 else pitch)
        tx, ty, tz = tvec.ravel() / 10.0
        raw_pose = np.array([tx, -ty, tz, -yaw, -pitch, -roll])
        self.pose = self.head_f(raw_pose, now)

        # ---- eyes: sub-pixel iris centre (circle fit of 5 iris points) in each eye's own frame
        eyes, opens = [], []
        for (o, i, u, d), ring, iris in zip(EYE_KEY, EYE_RING, IRIS):
            po, pi = pts[o, :2], pts[i, :2]
            L = np.linalg.norm(pi - po) + 1e-6
            ax = (pi - po) / L
            ay = np.array([-ax[1], ax[0]])
            pc = pts[list(iris), :2].mean(0)
            cen = pts[ring, :2].mean(0)                 # eye contour centroid, more stable than lids
            v = pc - cen
            gx, gy = np.dot(v, ax) / L, np.dot(v, ay) / L
            if o == 263:
                gx = -gx                                 # both eyes: + = towards nose
            eyes += [gx, gy]
            opens.append(np.linalg.norm(pts[u, :2] - pts[d, :2]) / L)
        op = float(np.mean(opens))
        self.open_ref = op if self.open_ref is None else 0.995 * self.open_ref + 0.005 * max(op, self.open_ref * 0.8)
        self.blink = op < 0.38 * self.open_ref
        if not self.blink:                               # freeze eye signal during blinks
            eyes = np.array(eyes)
            self._last_eyes = eyes
        elif getattr(self, "_last_eyes", None) is not None:
            eyes = self._last_eyes

        p = self.pose - self.center
        # confidence: eye pixel size, head angle, eyelids open
        ipd_px = np.linalg.norm(pts[468, :2] - pts[473, :2])
        c = np.clip(ipd_px / 60.0, 0, 1)
        c *= np.clip(1 - (abs(p[3]) - 30) / 20, 0, 1) * np.clip(1 - (abs(p[4]) - 20) / 15, 0, 1)
        c *= 0.0 if self.blink else np.clip(op / (self.open_ref + 1e-6) / 0.8, 0, 1)
        self.conf = float(c)
        if getattr(self, "brightness", 255) < 35:
            self.status = "Very dark - turn on a light in front of you (monitor glow alone isn't enough)"
        elif ipd_px < 45:
            self.status = "Face small in image - move closer or pick 1920x1080 in Settings"
        a = raw_pose                                     # unfiltered: no lag in the gaze path
        self.features = np.concatenate([eyes, [op / (self.open_ref + 1e-6)], a[3:6], a[0:3]])
        self.history.append((now, self.features))
        if len(self.history) > 120:
            del self.history[:30]

        # ---- game output
        s = self.s
        out = p * np.array([s["sens_pos"]] * 3 + [s["sens_yaw"], s["sens_pitch"], s["sens_roll"]])
        dz = s["deadzone"]
        out[3:] = np.sign(out[3:]) * np.maximum(np.abs(out[3:]) - dz, 0)
        self.out_pose = out
        if s["udp"]:
            try:
                self.sock.sendto(struct.pack("<6d", *out), ("127.0.0.1", int(s["udp_port"])))
            except OSError:
                pass

        if s["show_mesh"]:
            for a, b in CONTOURS:
                cv2.line(img, tuple(pts[a, :2].astype(int)), tuple(pts[b, :2].astype(int)),
                         (0, 184, 217), 1, cv2.LINE_AA)
            for iris in IRIS:
                c = pts[list(iris), :2].mean(0)
                r = np.linalg.norm(pts[iris[1], :2] - pts[iris[3], :2]) / 2
                cv2.circle(img, tuple(c.astype(int)), max(int(r), 2), (255, 80, 80), 1, cv2.LINE_AA)
                cv2.circle(img, tuple(c.astype(int)), 2, (255, 80, 80), -1, cv2.LINE_AA)
            nose = pts[1, :2]
            d = R @ np.array([0, 0, 1.0])
            cv2.line(img, tuple(nose.astype(int)),
                     tuple((nose + d[:2] * 90).astype(int)), (255, 0, 200), 3, cv2.LINE_AA)

    def features_at(self, t):
        for tt, f in reversed(self.history):
            if tt <= t:
                return f
        return None


# ---------------------------------------------------------------- gaze model
def design(F):
    F = np.atleast_2d(F)
    lx, ly, rx, ry, op, yaw, pit, rol, hx, hy, hz = F.T
    ex, ey = (lx + rx) / 2, (ly + ry) / 2
    return np.column_stack([
        np.ones(len(F)), lx, ly, rx, ry, op,
        ex * ex, ey * ey, ex * ey, ex * ey * ey, ex * ex * ey,    # eye nonlinearity
        yaw, pit, rol, hx, hy, hz,                                # head rotation + position
        ex * yaw, ey * pit])                                      # coupling terms


class GazeModel:
    """Ridge-regularised polynomial, lambda picked by leave-one-target-out cross-validation."""

    def __init__(self):
        self.W = self.mu = self.sd = None
        self.error = None
        self.F = self.Y = None            # calibration data
        self.CF, self.CY = [], []         # click-learning data
        try:
            d = json.load(open(CALIB_FILE))
            if len(d["mu"]) == N_FEAT:  # Legacy imports default to equal sample weights.
                self.F, self.Y = np.array(d["F"]), np.array(d["Y"])
                self.w = np.array(d.get("w", np.ones(len(self.F))))
                self.CF, self.CY = [np.array(x) for x in d.get("CF", [])], [tuple(y) for y in d.get("CY", [])]
                self.error = d.get("error")
                self._solve(d.get("lam", 1.0))
        except Exception:
            pass

    @staticmethod
    def _ridge(X, Y, lam, w=None):
        w = np.ones(len(X)) if w is None else w
        Xw = X * w[:, None]
        reg = lam * np.eye(X.shape[1])
        reg[0, 0] = 0
        return np.linalg.solve(Xw.T @ X + reg, Xw.T @ Y)

    def _data(self):
        F, Y, w = self.F, self.Y, self.w
        if self.CF:
            F = np.vstack([F, np.array(self.CF)])
            Y = np.vstack([Y, np.array(self.CY)])
            w = np.concatenate([w, np.full(len(self.CF), 2.0)])
        return F, Y, w

    def _solve(self, lam=None):
        F, Y, w = self._data()
        self.mu, self.sd = self.F.mean(0), self.F.std(0) + 1e-6
        X = design((F - self.mu) / self.sd)
        self.lam = lam if lam is not None else getattr(self, "lam", 1.0)
        self.W = self._ridge(X, Y, self.lam, w)
        # robust re-weighting: samples where you glanced away get down-weighted
        for _ in range(2):
            r = np.linalg.norm(X @ self.W - Y, axis=1)
            c = 2.5 * np.median(r) + 1e-6
            self.W = self._ridge(X, Y, self.lam, w * np.minimum(1.0, c / (r + 1e-6)))

    def fit(self, F, Y, groups):
        F, Y, groups = np.asarray(F), np.asarray(Y, float), np.asarray(groups)
        # eye-phase samples are the ground truth for "where on screen"; head-phase samples
        # only need to teach head compensation, so they count half
        w = np.where(groups >= 100, 0.5, 1.0)
        self.F, self.Y, self.w, self.CF, self.CY = F, Y, w, [], []
        mu, sd = F.mean(0), F.std(0) + 1e-6
        X = design((F - mu) / sd)
        best = (1e18, 1.0)
        eye_groups = [g for g in np.unique(groups) if g < 100]
        for lam in [0.01, 0.03, 0.1, 0.3, 1, 3, 10, 30, 100]:
            errs = []
            for g in np.unique(groups):
                tr, te = groups != g, groups == g
                W = self._ridge(X[tr], Y[tr], lam, w[tr])
                if g in eye_groups:
                    errs.append(np.median(np.linalg.norm(X[te] @ W - Y[te], axis=1)))
            best = min(best, (float(np.mean(errs)), lam))
        self.error = best[0]            # honest held-out error on targets not used in the fit
        self._solve(best[1])
        self.save()
        return self.error

    def add_click(self, f, xy):
        if self.W is None:
            return
        self.CF.append(np.asarray(f))
        self.CY.append(tuple(xy))
        self.CF, self.CY = self.CF[-80:], self.CY[-80:]
        self._solve()
        if len(self.CF) % 5 == 0:
            self.save()

    def save(self):
        json.dump({"F": self.F.tolist(), "Y": self.Y.tolist(), "w": self.w.tolist(), "mu": self.mu.tolist(),
                   "CF": [list(map(float, f)) for f in self.CF], "CY": self.CY,
                   "error": self.error, "lam": self.lam}, open(CALIB_FILE, "w"))

    def reset(self):
        self.W = self.F = self.Y = None
        self.CF, self.CY = [], []
        self.error = None
        if os.path.exists(CALIB_FILE):
            os.remove(CALIB_FILE)

    def predict(self, f):
        return (design((f - self.mu) / self.sd) @ self.W)[0]


# ---------------------------------------------------------------- UI helpers
def card(parent, title=None):
    f = ctk.CTkFrame(parent, fg_color=CARD, corner_radius=14)
    if title:
        ctk.CTkLabel(f, text=title, font=("Segoe UI Semibold", 15), text_color=TEXT
                     ).pack(anchor="w", padx=16, pady=(14, 6))
    return f


class Slider(ctk.CTkFrame):
    def __init__(self, parent, app, key, label, lo, hi, fmt="{:.2f}", cb=None):
        super().__init__(parent, fg_color="transparent")
        self.app, self.key, self.fmt, self.cb = app, key, fmt, cb
        top = ctk.CTkFrame(self, fg_color="transparent")
        top.pack(fill="x")
        ctk.CTkLabel(top, text=label, text_color=MUTED).pack(side="left")
        self.val = ctk.CTkLabel(top, text=fmt.format(app.s[key]), text_color=TEXT)
        self.val.pack(side="right")
        self.sl = ctk.CTkSlider(self, from_=lo, to=hi, command=self._set,
                                button_color=ACCENT, progress_color=ACCENT)
        self.sl.set(app.s[key])
        self.sl.pack(fill="x", pady=(2, 8))

    def _set(self, v):
        self.app.s[self.key] = float(v)
        self.val.configure(text=self.fmt.format(v))
        if self.cb:
            self.cb()
        self.app.save()


class Toggle(ctk.CTkFrame):
    def __init__(self, parent, app, key, label, desc="", cb=None):
        super().__init__(parent, fg_color="transparent")
        self.app, self.key, self.cb = app, key, cb
        txt = ctk.CTkFrame(self, fg_color="transparent")
        txt.pack(side="left", fill="x", expand=True)
        ctk.CTkLabel(txt, text=label, text_color=TEXT, font=("Segoe UI Semibold", 13)).pack(anchor="w")
        if desc:
            ctk.CTkLabel(txt, text=desc, text_color=MUTED, font=("Segoe UI", 11)).pack(anchor="w")
        self.sw = ctk.CTkSwitch(self, text="", command=self._set, progress_color=ACCENT)
        self.sw.pack(side="right")
        self.refresh()

    def _set(self):
        self.app.s[self.key] = bool(self.sw.get())
        if self.cb:
            self.cb()
        self.app.save()

    def refresh(self):
        (self.sw.select if self.app.s[self.key] else self.sw.deselect)()


# ---------------------------------------------------------------- app
class App(ctk.CTk):
    def __init__(self):
        ctk.set_appearance_mode("dark")
        super().__init__(fg_color=BG)
        self.title("OpenGaze")
        self.geometry("1180x740")
        self.minsize(980, 640)
        self.s = load_settings()
        self.sw_, self.sh_ = self.winfo_screenwidth(), self.winfo_screenheight()
        self.tr = Tracker(self.s)
        self.tr.apply_smoothing()
        self.model = GazeModel()
        self.gaze_f = self._new_gaze_filter()
        self._last_f = None
        self.target = self.render = None
        self._last_render = time.perf_counter()
        self.calibrating = False
        self.keystate = {}
        self.toggles = {}

        self._build()
        self._make_overlay()
        self.tr.start()
        self.protocol("WM_DELETE_WINDOW", self.quit_app)
        self.after(15, self.tick)

    def _new_gaze_filter(self):
        r = self.s["stability"]
        if self.model.W is not None and self.model.error:
            r = min(max(r, 0.5 * self.model.error), 90)
        return FixationFilter(r)

    def save(self):
        try:
            json.dump(self.s, open(SETTINGS_FILE, "w"), indent=2)
        except Exception:
            pass

    # ---------- layout
    def _build(self):
        self.grid_columnconfigure(1, weight=1)
        self.grid_rowconfigure(0, weight=1)

        side = ctk.CTkFrame(self, fg_color=PANEL, corner_radius=0, width=210)
        side.grid(row=0, column=0, sticky="ns")
        side.grid_propagate(False)
        ctk.CTkLabel(side, text="◉ OpenGaze", font=("Segoe UI Black", 22), text_color=ACCENT
                     ).pack(anchor="w", padx=20, pady=(24, 2))
        ctk.CTkLabel(side, text="Webcam head & eye tracking", text_color=MUTED,
                     font=("Segoe UI", 11)).pack(anchor="w", padx=20, pady=(0, 20))

        self.pages, self.navbtns = {}, {}
        host = ctk.CTkFrame(self, fg_color=BG)
        host.grid(row=0, column=1, sticky="nsew", padx=18, pady=18)
        host.grid_rowconfigure(0, weight=1)
        host.grid_columnconfigure(0, weight=1)
        for name, icon, builder in [("Home", "⌂", self._page_home),
                                    ("Calibration", "◎", self._page_calib),
                                    ("Games", "🎮", self._page_games),
                                    ("Settings", "⚙", self._page_settings)]:
            b = ctk.CTkButton(side, text=f"  {icon}   {name}", anchor="w", height=42,
                              corner_radius=10, fg_color="transparent", hover_color=CARD,
                              text_color=TEXT, font=("Segoe UI Semibold", 14),
                              command=lambda n=name: self.show(n))
            b.pack(fill="x", padx=12, pady=3)
            self.navbtns[name] = b
            page = ctk.CTkFrame(host, fg_color=BG)
            page.grid(row=0, column=0, sticky="nsew")
            if name == "Home":
                builder(page)
            else:
                sc = ctk.CTkScrollableFrame(page, fg_color=BG)
                sc.pack(fill="both", expand=True)
                builder(sc)
            self.pages[name] = page

        bottom = ctk.CTkFrame(side, fg_color="transparent")
        bottom.pack(side="bottom", fill="x", padx=16, pady=16)
        self.status_dot = ctk.CTkLabel(bottom, text="●", text_color="#f85149", font=("Segoe UI", 16))
        self.status_dot.pack(side="left")
        self.fps_lbl = ctk.CTkLabel(bottom, text="-- fps", text_color=MUTED, justify="left")
        self.fps_lbl.pack(side="left", padx=6)
        ctk.CTkLabel(side, text="F8 recenter · F9 pause · F10 mouse", text_color=MUTED,
                     font=("Segoe UI", 10)).pack(side="bottom", pady=(0, 4))
        self.show("Home")

    def show(self, name):
        self.pages[name].tkraise()
        for n, b in self.navbtns.items():
            b.configure(fg_color=CARD if n == name else "transparent")

    def _page_home(self, p):
        p.grid_columnconfigure(0, weight=3, uniform="h")
        p.grid_columnconfigure(1, weight=2, uniform="h")
        p.grid_rowconfigure(0, weight=1)

        cam = card(p)
        cam.grid(row=0, column=0, sticky="nsew", padx=(0, 12))
        self.preview = tk.Label(cam, bg="#000", bd=0)
        self._last_frame = None
        self.preview.pack(fill="both", expand=True, padx=12, pady=(12, 6))
        self.status_lbl = ctk.CTkLabel(cam, text="", text_color=MUTED)
        self.status_lbl.pack(anchor="w", padx=16, pady=(0, 10))

        right = ctk.CTkFrame(p, fg_color="transparent")
        right.grid(row=0, column=1, sticky="nsew")

        c = card(right, "Quick controls")
        c.pack(fill="x", pady=(0, 12))
        for key, label, desc in [("overlay", "Gaze bubble", "Show where you're looking"),
                                 ("mouse", "Mouse control", "Cursor follows gaze (F10)"),
                                 ("udp", "Game head tracking", "Send to opentrack")]:
            t = Toggle(c, self, key, label, desc, cb=self._apply_overlay)
            t.pack(fill="x", padx=16, pady=6)
            self.toggles[key] = t
        row = ctk.CTkFrame(c, fg_color="transparent")
        row.pack(fill="x", padx=16, pady=(8, 16))
        ctk.CTkButton(row, text="Calibrate", fg_color=ACCENT, hover_color="#0095b0",
                      text_color="#001014", font=("Segoe UI Semibold", 13),
                      command=self.calibrate).pack(side="left", expand=True, fill="x", padx=(0, 6))
        ctk.CTkButton(row, text="Recenter", fg_color=PANEL, hover_color="#26303b",
                      command=self.tr.recenter).pack(side="left", expand=True, fill="x", padx=(6, 0))

        h = card(right, "Head pose")
        h.pack(fill="both", expand=True)
        self.pose_cv = tk.Canvas(h, bg=CARD, highlightthickness=0, height=150)
        self.pose_cv.pack(fill="x", padx=16)
        self.pose_lbl = ctk.CTkLabel(h, text="", text_color=MUTED, font=("Consolas", 12), justify="left")
        self.pose_lbl.pack(anchor="w", padx=16, pady=(6, 14))

    def _page_calib(self, p):
        c = card(p, "Eye-gaze calibration")
        c.pack(fill="x", pady=(0, 12))
        ctk.CTkLabel(c, justify="left", text_color=MUTED, wraplength=700, text=(
            "Step 1: look at 20 dots with your head still (eyes). Step 2: keep your eyes on 5 dots "
            "while moving your head around (head). About 70 seconds total. "
            "The error shown is measured on targets held out of the fit, so it's honest.")).pack(anchor="w", padx=16)
        self.calib_lbl = ctk.CTkLabel(c, text="", text_color=TEXT, font=("Segoe UI Semibold", 14))
        self.calib_lbl.pack(anchor="w", padx=16, pady=10)
        row = ctk.CTkFrame(c, fg_color="transparent")
        row.pack(anchor="w", padx=16, pady=(0, 16))
        ctk.CTkButton(row, text="Start calibration", fg_color=ACCENT, text_color="#001014",
                      hover_color="#0095b0", command=self.calibrate).pack(side="left", padx=(0, 8))
        ctk.CTkButton(row, text="Test accuracy", fg_color=PANEL, hover_color="#26303b",
                      command=self.accuracy_test).pack(side="left", padx=(0, 8))
        ctk.CTkButton(row, text="Reset", fg_color=PANEL, hover_color="#5a1d1d",
                      command=self._reset_calib).pack(side="left")
        self._update_calib_lbl()
        Toggle(c, self, "learn_clicks", "Learn from my clicks",
               "Every mouse click quietly refines calibration (you look where you click)"
               ).pack(fill="x", padx=16, pady=(0, 16))

        g = card(p, "Gaze pointer")
        g.pack(fill="x")
        inner = ctk.CTkFrame(g, fg_color="transparent")
        inner.pack(fill="x", padx=16, pady=(0, 12))
        Slider(inner, self, "stability", "Fixation lock radius (bigger = steadier, less fine movement)", 10, 150,
               "{:.0f}px", cb=lambda: setattr(self, "gaze_f", self._new_gaze_filter())).pack(fill="x")
        Slider(inner, self, "gaze_smoothing", "Glide (0 = snappiest)", 0.0, 4.0).pack(fill="x")
        Slider(inner, self, "bubble_size", "Bubble size", 8, 80, "{:.0f}px").pack(fill="x")
        crow = ctk.CTkFrame(inner, fg_color="transparent")
        crow.pack(fill="x")
        ctk.CTkLabel(crow, text="Bubble colour", text_color=MUTED).pack(side="left")
        for col in ["#00e5ff", "#39d353", "#ff4d6d", "#ffd33d", "#ffffff"]:
            ctk.CTkButton(crow, text="", width=26, height=26, corner_radius=13, fg_color=col,
                          hover_color=col, command=lambda c_=col: self._set_color(c_)
                          ).pack(side="left", padx=4)

    def _page_games(self, p):
        c = card(p, "Game head tracking (opentrack)")
        c.pack(fill="x", pady=(0, 12))
        ctk.CTkLabel(c, justify="left", text_color=MUTED, wraplength=720, text=(
            "OpenGaze sends 6DOF head tracking to opentrack, which drives Microsoft Flight "
            "Simulator, DCS, Star Citizen, ETS2/ATS, iRacing, Elite Dangerous, Arma and other "
            "TrackIR/FreeTrack games.\n\n"
            "1. Install opentrack (free).   2. Input: 'UDP over network', same port as below.\n"
            "3. Output: the protocol for your game (freetrack 2.0 works for most).   4. Start, then press F8 in-game to recenter.")
        ).pack(anchor="w", padx=16)
        Toggle(c, self, "udp", "Send head tracking", "UDP 127.0.0.1").pack(fill="x", padx=16, pady=10)
        prow = ctk.CTkFrame(c, fg_color="transparent")
        prow.pack(anchor="w", padx=16, pady=(0, 16))
        ctk.CTkLabel(prow, text="Port", text_color=MUTED).pack(side="left", padx=(0, 8))
        self.port_e = ctk.CTkEntry(prow, width=90)
        self.port_e.insert(0, str(self.s["udp_port"]))
        self.port_e.pack(side="left")
        ctk.CTkButton(prow, text="Apply", width=70, fg_color=PANEL,
                      command=self._set_port).pack(side="left", padx=8)

        s = card(p, "Sensitivity (game output)")
        s.pack(fill="x")
        inner = ctk.CTkFrame(s, fg_color="transparent")
        inner.pack(fill="x", padx=16, pady=(0, 12))
        for k, l in [("sens_yaw", "Yaw (turn)"), ("sens_pitch", "Pitch (nod)"),
                     ("sens_roll", "Roll (tilt)"), ("sens_pos", "Position (lean)")]:
            Slider(inner, self, k, l, 0.2, 6.0, "{:.1f}×").pack(fill="x")
        Slider(inner, self, "deadzone", "Deadzone", 0.0, 5.0, "{:.1f}°").pack(fill="x")

    def _page_settings(self, p):
        c = card(p, "Camera")
        c.pack(fill="x", pady=(0, 12))
        self.cams = list_cameras()
        row = ctk.CTkFrame(c, fg_color="transparent")
        row.pack(fill="x", padx=16, pady=(0, 16))
        cur = self.cams[self.s["camera"]] if self.s["camera"] < len(self.cams) else self.cams[0]
        ctk.CTkOptionMenu(row, values=self.cams, command=self._set_cam, fg_color=PANEL,
                          button_color=PANEL, variable=ctk.StringVar(value=cur), width=300
                          ).pack(side="left", padx=(0, 10))
        ctk.CTkOptionMenu(row, values=["640x480", "1280x720", "1920x1080", "2560x1440"], fg_color=PANEL,
                          button_color=PANEL, variable=ctk.StringVar(value=self.s["resolution"]),
                          command=self._set_res).pack(side="left")

        t = card(p, "Tracking")
        t.pack(fill="x", pady=(0, 12))
        inner = ctk.CTkFrame(t, fg_color="transparent")
        inner.pack(fill="x", padx=16, pady=(0, 12))
        Slider(inner, self, "smoothing", "Head smoothing (higher = steadier, more lag)", 0.2, 5.0,
               cb=self.tr.apply_smoothing).pack(fill="x")
        Slider(inner, self, "head_mouse_yaw", "Head-mouse range, horizontal (uncalibrated)", 5, 45, "{:.0f}°").pack(fill="x")
        Slider(inner, self, "head_mouse_pitch", "Head-mouse range, vertical (uncalibrated)", 5, 30, "{:.0f}°").pack(fill="x")
        Toggle(inner, self, "show_mesh", "Draw face mesh in preview").pack(fill="x", pady=6)
        Toggle(inner, self, "fast_exposure", "Force high frame rate (manual exposure)",
               "Off = camera's own auto exposure (recommended). On only if you have bright light and want 60 fps", cb=self._restart_cam).pack(fill="x", pady=6)
        self.cam_info_lbl = ctk.CTkLabel(inner, text="", text_color=MUTED)
        self.cam_info_lbl.pack(anchor="w")
        Toggle(inner, self, "low_light", "Low-light / far-camera boost",
               "Local contrast enhancement on the face before tracking").pack(fill="x", pady=6)

        a = card(p, "About")
        a.pack(fill="x")
        ctk.CTkLabel(a, text="OpenGaze · free, fully local, no account. Your camera feed never leaves this PC.",
                     text_color=MUTED).pack(anchor="w", padx=16, pady=(0, 16))

    # ---------- setting callbacks
    def _set_cam(self, name):
        self.s["camera"] = self.cams.index(name)
        self.tr.restart = True
        self.save()

    def _restart_cam(self):
        self.tr._attempt = 0
        self.tr.restart = True

    def _set_res(self, r):
        self.s["resolution"] = r
        self.tr.restart = True
        self.save()

    def _set_port(self):
        try:
            self.s["udp_port"] = int(self.port_e.get())
            self.save()
        except ValueError:
            pass

    def _set_color(self, c):
        self.s["bubble_color"] = c
        self.ov_canvas.itemconfigure(self.dot, outline=c)
        self.save()

    def _reset_calib(self):
        self.model.reset()
        self.tr.model3d = None                 # rebuild face model on next frontal frame
        if os.path.exists(FACE_MODEL_FILE):
            os.remove(FACE_MODEL_FILE)
        self._update_calib_lbl()

    def _update_calib_lbl(self):
        if self.model.W is None:
            self.calib_lbl.configure(text="Not calibrated — the pointer follows your head instead.")
        else:
            e = self.model.error
            self.calib_lbl.configure(text=f"✓ Calibrated" + (f" · cross-validated error ≈ {e:.0f}px" if e else ""))

    def _apply_overlay(self):
        for t in self.toggles.values():
            t.refresh()

    # ---------- overlay
    def _make_overlay(self):
        o = tk.Toplevel(self)
        o.overrideredirect(True)
        o.attributes("-topmost", True)
        o.geometry(f"{self.sw_}x{self.sh_}+0+0")
        o.configure(bg="#010101")
        o.attributes("-transparentcolor", "#010101")
        self.ov_canvas = tk.Canvas(o, bg="#010101", highlightthickness=0)
        self.ov_canvas.pack(fill="both", expand=True)
        self.dot = self.ov_canvas.create_oval(0, 0, 0, 0, outline=self.s["bubble_color"], width=3)
        self.acc_items = []
        o.update_idletasks()
        hwnd = ctypes.windll.user32.GetParent(o.winfo_id())
        st = ctypes.windll.user32.GetWindowLongW(hwnd, -20)
        ctypes.windll.user32.SetWindowLongW(hwnd, -20, st | 0x80000 | 0x20 | 0x80)  # layered, click-through, toolwindow
        self.overlay = o

    # ---------- main loop
    def _hotkeys(self):
        for vk, fn in [(0x77, self.tr.recenter),                                         # F8
                       (0x78, lambda: setattr(self.tr, "paused", not self.tr.paused)),   # F9
                       (0x79, self._toggle_mouse),
                       (0x01, self._learn_click)]:                                       # left click
            down = bool(ctypes.windll.user32.GetAsyncKeyState(vk) & 0x8000)
            if down and not self.keystate.get(vk):
                fn()
            self.keystate[vk] = down

    def _toggle_mouse(self):
        self.s["mouse"] = not self.s["mouse"]
        self._apply_overlay()
        self.save()

    def tick(self):
        self._hotkeys()
        tr = self.tr
        f = tr.features
        if f is not None and tr.face and not tr.paused and not self.calibrating:
            now = time.perf_counter()
            if self.model.W is not None:
                xy = self.model.predict(f)
            else:
                p = tr.pose - tr.center
                xy = np.array([self.sw_ / 2 + p[3] / self.s["head_mouse_yaw"] * self.sw_ / 2,
                               self.sh_ / 2 - p[4] / self.s["head_mouse_pitch"] * self.sh_ / 2])
            if f is not self._last_f:          # new camera sample -> update fixation target
                self._last_f = f
                self.target = self.gaze_f(np.clip(xy, 0, [self.sw_ - 1, self.sh_ - 1]), tr.conf)
            # render at display rate, gliding to the target (fast but never jumpy)
            dt = now - self._last_render
            self._last_render = now
            k = 1 - math.exp(-dt / (0.012 + 0.02 * self.s["gaze_smoothing"]))
            self.render = self.target if self.render is None else self.render + (self.target - self.render) * k
            xy = self.gaze = self.render
            x, y, r = int(xy[0]), int(xy[1]), int(self.s["bubble_size"])
            if self.s["overlay"]:
                self.ov_canvas.coords(self.dot, x - r, y - r, x + r, y + r)
            else:
                self.ov_canvas.coords(self.dot, -9, -9, -9, -9)
            if self.s["mouse"]:
                ctypes.windll.user32.SetCursorPos(x, y)
        else:
            self.ov_canvas.coords(self.dot, -9, -9, -9, -9)

        self._tick_ui()
        self.after(4, self.tick)

    def _tick_ui(self):
        self._ui_n = getattr(self, "_ui_n", 0) + 1
        if self._ui_n % 6:
            return  # UI refresh ~40 Hz, tracking loop stays at full rate
        tr = self.tr
        ok = tr.face and not tr.paused
        self.status_dot.configure(text_color="#3fb950" if ok else ("#d29922" if tr.frame is not None else "#f85149"))
        self.fps_lbl.configure(text=f"cam {tr.cam_fps:.0f} · track {tr.fps:.0f} fps\n"
                                    f"{tr.latency * 1000:.0f} ms · conf {tr.conf * 100:.0f}%")
        self.status_lbl.configure(text=tr.status)
        self.cam_info_lbl.configure(text=f"Camera: {tr.cam_info} · {tr.cam_fps:.0f} fps delivered · "
                                         f"brightness {getattr(tr, 'brightness', 0):.0f}/255 · "
                                         f"exposure {getattr(tr, '_exp', 0):.0f}")
        fr = tr.frame
        if fr is not None and self.pages["Home"].winfo_ismapped():
            pw, ph = max(self.preview.winfo_width() - 4, 100), max(self.preview.winfo_height() - 4, 100)
            if fr is not self._last_frame:
                self._last_frame = fr
                h, w = fr.shape[:2]
                sc = min(pw / w, ph / h)
                img = Image.fromarray(cv2.resize(fr, (int(w * sc), int(h * sc)), interpolation=cv2.INTER_LINEAR))
                self._photo = ImageTk.PhotoImage(img)
                self.preview.configure(image=self._photo)
        self._draw_pose()

    def _draw_pose(self):
        cv = self.pose_cv
        cv.delete("all")
        W = cv.winfo_width() or 300
        H = 150
        if self.tr.pose is None:
            return
        p = self.tr.pose - self.tr.center
        cx, cy, R = 80, H / 2, 60
        cv.create_oval(cx - R, cy - R, cx + R, cy + R, outline="#2d3742", width=2)
        cv.create_line(cx - R, cy, cx + R, cy, fill="#2d3742")
        cv.create_line(cx, cy - R, cx, cy + R, fill="#2d3742")
        dx = np.clip(p[3] / 45, -1, 1) * R
        dy = -np.clip(p[4] / 30, -1, 1) * R
        cv.create_oval(cx + dx - 8, cy + dy - 8, cx + dx + 8, cy + dy + 8, fill=ACCENT, outline="")
        # roll gauge
        rx = 160 + (W - 180) / 2
        a = math.radians(p[5])
        L = min(70, (W - 200) / 2)
        cv.create_line(rx - L * math.cos(a), cy - L * math.sin(a), rx + L * math.cos(a),
                       cy + L * math.sin(a), fill=ACCENT, width=4, capstyle="round")
        cv.create_text(rx, cy + 40, text="roll", fill=MUTED)
        self.pose_lbl.configure(text=f"yaw {p[3]:+6.1f}°   pitch {p[4]:+6.1f}°   roll {p[5]:+6.1f}°\n"
                                     f"x {p[0]:+5.1f}cm    y {p[1]:+5.1f}cm     z {p[2]:+5.1f}cm")

    # ---------- calibration
    def _fullscreen(self):
        win = tk.Toplevel(self)
        win.overrideredirect(True)
        win.attributes("-topmost", True)
        win.geometry(f"{self.sw_}x{self.sh_}+0+0")
        cv = tk.Canvas(win, bg="#0b0e12", highlightthickness=0)
        cv.pack(fill="both", expand=True)
        win.bind("<Escape>", lambda e: (win.destroy(), setattr(self, "calibrating", False)))
        win.focus_force()
        return win, cv

    def _run_targets(self, pts, on_sample, on_done, title, dur=2.0, settle=0.8, hint=None, gid0=0, win=None,
                     heading="", steps=()):
        if win is None:
            win, cv = self._fullscreen()
        else:
            cv = win.cv
            cv.delete("all")
        win.cv = cv
        cx, cy = self.sw_ / 2, self.sh_ / 2
        # ---- instruction screen: stays until you're ready
        cv.create_text(cx, cy - 170, text=heading or title, fill=TEXT, font=("Segoe UI Semibold", 30), tags="title")
        if heading:
            cv.create_text(cx, cy - 120, text=title, fill="#9aa7b4", font=("Segoe UI", 16), tags="title")
        for k, line in enumerate(steps):
            cv.create_text(cx - 330, cy - 60 + k * 42, text=f"{k + 1}.", fill=ACCENT, anchor="w",
                           font=("Segoe UI Semibold", 17), tags="title")
            cv.create_text(cx - 300, cy - 60 + k * 42, text=line, fill=TEXT, anchor="w",
                           font=("Segoe UI", 16), tags="title")
        yb = cy - 60 + max(len(steps), 1) * 42 + 50
        cv.create_rectangle(cx - 190, yb - 26, cx + 190, yb + 26, fill=ACCENT, outline="", tags="title")
        cv.create_text(cx, yb, text="Press SPACE or click to start", fill="#001014",
                       font=("Segoe UI Semibold", 16), tags="title")
        cv.create_text(cx, yb + 50, text="Esc to cancel", fill="#5c6875", font=("Segoe UI", 12), tags="title")
        short = (heading or title).split(".")[0]

        def show(i):
            if not win.winfo_exists():
                return
            if i == len(pts):
                return on_done(win)
            cv.delete("title")
            cv.delete("banner")
            cv.create_text(cx, 28, text=f"{short}   ·   dot {i + 1} of {len(pts)}", fill="#9aa7b4",
                           font=("Segoe UI Semibold", 14), tags="banner")
            if hint is None:
                cv.create_text(cx, 54, text="Look at the white centre of the dot. Keep your head still.",
                               fill="#5c6875", font=("Segoe UI", 12), tags="banner")
            x, y = pts[i][0] * self.sw_, pts[i][1] * self.sh_
            t0 = time.perf_counter()

            def step():
                if not win.winfo_exists():
                    return
                t = time.perf_counter() - t0
                cv.delete("dot")
                if hint:
                    # head phase: fixed small dot, progress ring, live head-movement trace
                    cv.create_oval(x - 9, y - 9, x + 9, y + 9, fill=ACCENT, outline="", tags="dot")
                    cv.create_oval(x - 3, y - 3, x + 3, y + 3, fill="#fff", outline="", tags="dot")
                    cv.create_arc(x - 40, y - 40, x + 40, y + 40, start=90, extent=-360 * min(t / dur, 0.999),
                                  style="arc", outline=ACCENT, width=3, tags="dot")
                    ty_ = y + 70 if y < self.sh_ / 2 else y - 70
                    cv.create_text(x, ty_, text=hint, fill="#9aa7b4", font=("Segoe UI", 14), tags="dot")
                    if self.tr.pose is not None:
                        pz = self.tr.pose - self.tr.center
                        hx, hy = x + np.clip(pz[3], -25, 25) * 3, y - np.clip(pz[4], -20, 20) * 3
                        cv.create_oval(hx - 4, hy - 4, hx + 4, hy + 4, outline="#ffd33d", width=2, tags="dot")
                else:
                    r = max(6, 30 - t * 20)
                    cv.create_oval(x - r, y - r, x + r, y + r, fill=ACCENT, outline="", tags="dot")
                    cv.create_oval(x - 3, y - 3, x + 3, y + 3, fill="#fff", outline="", tags="dot")
                if t > settle and self.tr.features is not None and not self.tr.blink:
                    on_sample(self.tr.features.copy(), (x, y), gid0 + i)
                if t < dur:
                    win.after(16, step)
                else:
                    show(i + 1)
            step()

        started = {"v": False}

        def start(_=None):
            if not started["v"]:
                started["v"] = True
                cv.delete("title")
                cv.create_text(cx, cy, text="Get ready...", fill="#9aa7b4", font=("Segoe UI", 22), tags="title")
                win.after(700, lambda: show(0))

        win.bind("<space>", start)
        win.bind("<Button-1>", start)
        win.focus_force()

    def calibrate(self):
        """Two-phase calibration.
        Phase 1 (eyes):  20 dots across the screen, head still -> learns the eye->screen map.
        Phase 2 (head):  5 dots, eyes stay on the dot while you move your head around ->
                         teaches the model exactly how head turns/leans shift the eye signal,
                         so the gaze stays put when you move your head during real use."""
        if self.calibrating:
            return
        if self.tr.model3d is None:
            self.calib_lbl.configure(text="Look straight at the camera first so your face model can be built.")
            return
        self.calibrating = True
        self.tr.recenter()
        xs, ys = np.linspace(0.05, 0.95, 5), np.linspace(0.07, 0.93, 4)
        eye_pts = [(x, y) for y in ys for x in xs]
        np.random.shuffle(eye_pts)
        head_pts = [(0.5, 0.5), (0.2, 0.22), (0.8, 0.22), (0.8, 0.78), (0.2, 0.78)]
        F, Y, G = [], [], []
        add = lambda f, y, i: (F.append(f), Y.append(y), G.append(i))

        def phase2(win):
            self._run_targets(head_pts, add, finish, "Teaches the tracker how your head moves affect your eyes.",
                              dur=5.0, settle=0.6, gid0=100, win=win,
                              hint="keep looking here - move your head around",
                              heading="Step 2 of 2 - Head",
                              steps=["5 dots will appear, one at a time (5 seconds each).",
                                     "Keep your EYES locked on the dot the whole time.",
                                     "Slowly move your HEAD: left, right, up, down, lean in and out.",
                                     "The yellow circle shows your head. The ring shows time left."])

        def finish(win):
            win.destroy()
            self.calibrating = False
            F_, Y_, G_ = np.array(F), np.array(Y), np.array(G)
            if len(F_) < 250:
                self.calib_lbl.configure(text="Calibration failed - face not visible enough. Try better lighting.")
                return
            keep = np.ones(len(F_), bool)
            for g in np.unique(G_):
                if g >= 100:          # head phase: eye signal is *supposed* to vary, keep it all
                    continue
                m = G_ == g           # eye phase: drop samples where eyes hadn't settled on the target
                e = F_[m][:, :4]
                d = np.abs(e - np.median(e, 0)).sum(1)
                keep[np.where(m)[0][d > np.percentile(d, 75) * 1.5 + 1e-9]] = False
            self.model.fit(F_[keep], Y_[keep], G_[keep])
            self.gaze_f = self._new_gaze_filter()
            self._update_calib_lbl()

        self._run_targets(eye_pts, add, phase2, "Teaches the tracker where your eyes point on the screen.",
                          heading="Step 1 of 2 - Eyes",
                          steps=["20 dots will appear, one at a time (2 seconds each).",
                                 "Look at the white centre of each dot until it shrinks away.",
                                 "Keep your head still - only move your eyes.",
                                 "Blinking is fine; those frames are ignored automatically."])

    def accuracy_test(self):
        if self.model.W is None or self.calibrating:
            return
        self.calibrating = True
        pts = [(.25, .25), (.75, .25), (.5, .5), (.25, .75), (.75, .75), (.5, .15), (.5, .85)]
        res = {}

        def sample(f, y, i):
            res.setdefault(i, []).append(self.model.predict(f))

        def done(w0):
            w0.destroy()
            self.calibrating = False
            if not res:
                return
            win, cv = self._fullscreen()
            errs = []
            for i, preds in res.items():
                tx, ty = pts[i][0] * self.sw_, pts[i][1] * self.sh_
                m = np.median(np.array(preds), 0)
                errs.append(np.linalg.norm(m - (tx, ty)))
                cv.create_oval(tx - 8, ty - 8, tx + 8, ty + 8, outline="#fff", width=2)
                for p in preds[::3]:
                    cv.create_oval(p[0] - 2, p[1] - 2, p[0] + 2, p[1] + 2, fill="#3a6b7a", outline="")
                cv.create_line(tx, ty, m[0], m[1], fill=ACCENT, width=2)
                cv.create_oval(m[0] - 5, m[1] - 5, m[0] + 5, m[1] + 5, fill=ACCENT, outline="")
            e = float(np.median(errs))
            deg = math.degrees(math.atan(self._px_mm() * e / 600))
            msg = f"Accuracy: median error {e:.0f}px  (~{deg:.1f} deg visual angle)"
            cv.create_text(self.sw_ / 2, 40, text=msg + "   -   click or press Esc to close",
                           fill=TEXT, font=("Segoe UI Semibold", 16))
            win.bind("<Button-1>", lambda e_: win.destroy())
            self.calib_lbl.configure(text=msg)

        self._run_targets(pts, sample, done, "Measures how accurate your calibration is.",
                          heading="Accuracy test",
                          steps=["7 dots will appear, one at a time.",
                                 "Look at the white centre of each dot, as naturally as you would normally.",
                                 "Afterwards you'll see where the tracker thought you looked."])

    def _px_mm(self):
        try:
            hdc = ctypes.windll.user32.GetDC(0)
            mm = ctypes.windll.gdi32.GetDeviceCaps(hdc, 4)   # HORZSIZE
            ctypes.windll.user32.ReleaseDC(0, hdc)
            if mm > 100:
                return mm / self.sw_
        except Exception:
            pass
        return 0.28 * 1920 / self.sw_

    def _learn_click(self):
        """Implicit recalibration: when you click, you're almost always looking at the cursor."""
        if not self.s["learn_clicks"] or self.s["mouse"] or self.model.W is None or self.calibrating:
            return
        f = self.tr.features_at(time.perf_counter() - 0.12)
        if f is None or self.tr.blink:
            return
        pt = ctypes.wintypes.POINT()
        ctypes.windll.user32.GetCursorPos(ctypes.byref(pt))
        wx, wy = self.winfo_rootx(), self.winfo_rooty()
        if wx <= pt.x <= wx + self.winfo_width() and wy <= pt.y <= wy + self.winfo_height():
            return   # clicks on our own UI are not gaze labels
        if np.linalg.norm(self.model.predict(f) - (pt.x, pt.y)) > 0.12 * self.sw_:
            return   # clicked without looking - ignore
        self.model.add_click(f, (pt.x, pt.y))

    def quit_app(self):
        self.tr.running = False
        self.save()
        self.after(150, self.destroy)


if __name__ == "__main__":
    App().mainloop()
