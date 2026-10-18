import { signal, effect, cleanup, Text, View } from "nsh";
import { Canvas } from "nsh/canvas";
import { keymap } from "nsh/keymap";

const W = 320, H = 480, GROUND = 420, PW = 52, BX = 80, BR = 11;
const GRAV = 1100, FLAP = -360, S = "R105";

function hitRect(cx: number, cy: number, r: number, x: number, y: number, w: number, h: number) {
  const nx = Math.max(x, Math.min(cx, x + w)), ny = Math.max(y, Math.min(cy, y + h));
  const dx = cx - nx, dy = cy - ny;
  return dx * dx + dy * dy < r * r;
}

export default function Game() {
  const phase = signal("ready");
  const score = signal(0);
  const best = signal(0);
  const muted = signal(false);
  const tick = signal(0);

  const bird = { y: H * 0.45, vy: 0, rot: 0 };
  let pipes: any[] = [], parts: any[] = [];
  let t = 0, gx = 0, overAt = 0, last = Date.now(), ac: any = null;
  const clouds = Array.from({ length: 5 }, (_, i) => ({ x: i * 90 + Math.random() * 40, y: 50 + Math.random() * 150, s: 0.8 + Math.random() * 0.6 }));

  const beep = (f: number, d: number, type: string, slide = 0) => {
    if (muted()) return;
    try {
      const g0: any = globalThis as any;
      const Ctor = g0.AudioContext || g0.webkitAudioContext;
      ac = ac || new Ctor();
      const o = ac.createOscillator(), g = ac.createGain(), n = ac.currentTime;
      o.type = type; o.frequency.setValueAtTime(f, n);
      if (slide) o.frequency.exponentialRampToValueAtTime(f + slide, n + d);
      g.gain.setValueAtTime(0.06, n); g.gain.exponentialRampToValueAtTime(0.001, n + d);
      o.connect(g).connect(ac.destination); o.start(); o.stop(n + d);
    } catch {}
  };

  const burst = (x: number, y: number, color: string, n: number) => {
    for (let i = 0; i < n; i++) {
      const a = Math.random() * Math.PI * 2, s = 60 + Math.random() * 160;
      parts.push({ x, y, vx: Math.cos(a) * s, vy: Math.sin(a) * s - 40, life: 0.6, color });
    }
  };

  const save = () => {
    const snap = JSON.stringify({ best: best() });
    fetch(`http://127.0.0.1:8787/state/${S}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: snap }).catch(() => {});
  };

  const reset = () => {
    bird.y = H * 0.45; bird.vy = 0; bird.rot = 0;
    pipes = []; parts = []; score.set(0);
  };

  const flap = () => {
    const p = phase();
    if (p === "over") {
      if (Date.now() - overAt < 350) return;
      reset(); phase.set("play");
    } else if (p === "ready") {
      reset(); phase.set("play");
    }
    if (phase() === "play") {
      bird.vy = FLAP;
      beep(480, 0.12, "square", 240);
    }
  };

  const die = () => {
    phase.set("over"); overAt = Date.now();
    burst(BX, bird.y, "#ffe066", 24);
    beep(180, 0.35, "sawtooth", -120);
    if (score() > best()) { best.set(score()); save(); }
  };

  const toggleMute = () => muted.update(m => !m);

  keymap.bind({ space: flap, enter: flap, m: toggleMute }, { repeat: false });

  effect(() => {
    fetch(`http://127.0.0.1:8787/state/${S}`).then(r => r.json()).then(d => {
      if (d && d.state && typeof d.state.best === "number") best.set(d.state.best);
    }).catch(() => {});
  });

  effect(() => {
    const iv = setInterval(() => {
      const now = Date.now();
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      if ((globalThis as any).document?.hidden) return;
      t += dt;
      for (const c of clouds) { c.x -= 18 * c.s * dt; if (c.x < -60) { c.x = W + 40; c.y = 50 + Math.random() * 150; } }
      const p = phase();
      const sc = score();
      const speed = 140 + Math.min(sc, 20) * 5;

      if (p === "ready") {
        bird.y = H * 0.45 + Math.sin(t * 4) * 6; bird.rot = 0;
      } else if (p === "play") {
        gx -= speed * dt;
        bird.vy += GRAV * dt; bird.y += bird.vy * dt;
        bird.rot = Math.max(-0.5, Math.min(1.2, bird.vy / 400));
        for (const q of pipes) q.x -= speed * dt;
        const lastPipe = pipes[pipes.length - 1];
        if (!lastPipe || lastPipe.x < W - 175) {
          const gap = Math.max(112, 150 - sc * 2);
          const gy = gap / 2 + 40 + Math.random() * (GROUND - gap - 80);
          pipes.push({ x: W + 10, gy, gap, passed: false });
        }
        pipes = pipes.filter(q => q.x + PW > -10);
        for (const q of pipes) {
          if (!q.passed && q.x + PW < BX - BR) {
            q.passed = true; score.update(n => n + 1);
            burst(BX + 10, bird.y, "#ffffff", 10);
            beep(880, 0.08, "triangle", 300);
          }
          const top = q.gy - q.gap / 2, bot = q.gy + q.gap / 2;
          if (hitRect(BX, bird.y, BR, q.x, 0, PW, top) || hitRect(BX, bird.y, BR, q.x, bot, PW, GROUND - bot)) { die(); break; }
        }
        if (phase() === "play" && (bird.y - BR < 0 || bird.y + BR >= GROUND)) {
          bird.y = Math.max(BR, Math.min(GROUND - BR, bird.y)); die();
        }
      } else if (p === "over") {
        if (bird.y + BR < GROUND) { bird.vy += GRAV * dt; bird.y += bird.vy * dt; bird.rot = Math.min(1.5, bird.rot + 4 * dt); }
        else { bird.y = GROUND - BR; bird.rot = 1.5; }
      }

      for (const q of parts) { q.x += q.vx * dt; q.y += q.vy * dt; q.vy += 300 * dt; q.life -= dt; }
      parts = parts.filter(q => q.life > 0);
      tick.update(n => n + 1);
    }, 16);
    cleanup(() => clearInterval(iv));
  });

  const drawBird = (ctx: any, flapping: boolean) => {
    ctx.save();
    ctx.translate(BX, bird.y); ctx.rotate(bird.rot);
    ctx.fillStyle = "#ffd23f"; ctx.beginPath(); ctx.arc(0, 0, BR + 2, 0, Math.PI * 2); ctx.fill();
    ctx.fillStyle = "#ffef9e"; ctx.beginPath(); ctx.ellipse(-2, 4, 7, 4, 0, 0, Math.PI * 2); ctx.fill();
    const wy = flapping ? Math.sin(t * 22) * 5 : 0;
    ctx.fillStyle = "#f7a823"; ctx.beginPath(); ctx.ellipse(-4, 2 + wy, 7, 4.5, -0.3, 0, Math.PI * 2); ctx.fill();
    ctx.fillStyle = "#fff"; ctx.beginPath(); ctx.arc(5, -4, 4.5, 0, Math.PI * 2); ctx.fill();
    ctx.fillStyle = "#222"; ctx.beginPath(); ctx.arc(6, -4, 2, 0, Math.PI * 2); ctx.fill();
    ctx.fillStyle = "#ff7a2f"; ctx.beginPath(); ctx.moveTo(9, -1); ctx.lineTo(18, 2); ctx.lineTo(9, 5); ctx.fill();
    ctx.fillStyle = "#ff8fa3"; ctx.beginPath(); ctx.arc(2, 5, 2.5, 0, Math.PI * 2); ctx.fill();
    ctx.restore();
  };

  const drawPipe = (ctx: any, x: number, y: number, w: number, h: number, cap: boolean) => {
    const g = ctx.createLinearGradient(x, 0, x + w, 0);
    g.addColorStop(0, "#3d9e2a"); g.addColorStop(0.35, "#5cb83a"); g.addColorStop(1, "#2a6e1c");
    ctx.fillStyle = g; ctx.fillRect(x, y, w, h);
    ctx.fillStyle = "rgba(255,255,255,0.18)"; ctx.fillRect(x + 6, y, 5, h);
    if (cap) {
      const cy = y + h - 22;
      ctx.fillStyle = g; ctx.fillRect(x - 4, cy, w + 8, 22);
      ctx.fillStyle = "rgba(0,0,0,0.15)"; ctx.fillRect(x - 4, cy + 18, w + 8, 4);
    } else {
      ctx.fillStyle = g; ctx.fillRect(x - 4, y, w + 8, 22);
    }
  };

  return (
    <View class="flex flex-col items-center gap-3 p-2">
      <View onPress={flap} activeOpacity={0.9}>
        <Canvas
          style={{ width: W, height: H }}
          draw={(ctx: any, size: any) => {
            tick();
            const p = phase(), sc = score(), bs = best(), m = muted();
            const sx = (size?.width || W) / W, sy = (size?.height || H) / H;
            ctx.save();
            ctx.scale(sx, sy);
            ctx.imageSmoothingEnabled = false;

            const sky = ctx.createLinearGradient(0, 0, 0, GROUND);
            sky.addColorStop(0, "#4ec3ff"); sky.addColorStop(1, "#bfeaff");
            ctx.fillStyle = sky; ctx.fillRect(0, 0, W, H);

            ctx.fillStyle = "#fff";
            for (const c of clouds) {
              ctx.beginPath();
              ctx.arc(c.x, c.y, 18 * c.s, 0, Math.PI * 2);
              ctx.arc(c.x + 20 * c.s, c.y - 8 * c.s, 22 * c.s, 0, Math.PI * 2);
              ctx.arc(c.x + 42 * c.s, c.y, 16 * c.s, 0, Math.PI * 2);
              ctx.fill();
            }

            const hillOff = (t * 25) % 160;
            ctx.fillStyle = "#8fd96b";
            for (let x = -160; x < W + 160; x += 160) {
              ctx.beginPath(); ctx.arc(x - hillOff + 60, GROUND + 10, 90, Math.PI, 0); ctx.fill();
            }

            for (const q of pipes) {
              const top = q.gy - q.gap / 2, bot = q.gy + q.gap / 2;
              drawPipe(ctx, q.x, 0, PW, top, true);
              drawPipe(ctx, q.x, bot, PW, GROUND - bot, true);
            }

            ctx.fillStyle = "#7ddc4f"; ctx.fillRect(0, GROUND, W, 12);
            ctx.fillStyle = "#e8c27a"; ctx.fillRect(0, GROUND + 12, W, H - GROUND - 12);
            ctx.fillStyle = "#d4a95c";
            const stripe = ((gx % 24) + 24) % 24;
            for (let x = stripe - 24; x < W + 24; x += 24) ctx.fillRect(x, GROUND + 22, 12, 4);

            for (const q of parts) {
              ctx.globalAlpha = Math.max(0, q.life / 0.6);
              ctx.fillStyle = q.color; ctx.fillRect(q.x, q.y, 4, 4);
            }
            ctx.globalAlpha = 1;

            drawBird(ctx, p !== "over");

            ctx.textAlign = "center";
            if (p === "play" || p === "over") {
              ctx.font = "bold 46px monospace";
              ctx.lineWidth = 6; ctx.strokeStyle = "#2a2a4a"; ctx.strokeText(String(sc), W / 2, 70);
              ctx.fillStyle = "#fff"; ctx.fillText(String(sc), W / 2, 70);
            }
            if (p === "ready") {
              const bob = Math.sin(t * 4) * 4;
              ctx.font = "bold 30px monospace";
              ctx.lineWidth = 6; ctx.strokeStyle = "#2a2a4a"; ctx.strokeText("TAP TO FLY", W / 2, 140 + bob);
              ctx.fillStyle = "#ffe066"; ctx.fillText("TAP TO FLY", W / 2, 140 + bob);
            }
            if (p === "over") {
              ctx.fillStyle = "rgba(20,30,50,0.55)"; ctx.fillRect(40, 140, W - 80, 190);
              ctx.strokeStyle = "#ffe066"; ctx.lineWidth = 3; ctx.strokeRect(40, 140, W - 80, 190);
              ctx.font = "bold 28px monospace"; ctx.fillStyle = "#ff6b6b"; ctx.fillText("GAME OVER", W / 2, 180);
              ctx.font = "bold 18px monospace"; ctx.fillStyle = "#fff";
              ctx.fillText("SCORE  " + sc, W / 2, 222);
              ctx.fillText("BEST   " + bs, W / 2, 252);
              ctx.font = "bold 16px monospace"; ctx.fillStyle = "#ffe066";
              ctx.fillText(Date.now() - overAt > 350 ? "TAP OR SPACE TO RETRY" : "...", W / 2, 300);
            }
            ctx.textAlign = "left";
            ctx.font = "bold 12px monospace"; ctx.fillStyle = "#fff";
            ctx.fillText("BEST " + bs, 10, 22);
            ctx.textAlign = "right";
            ctx.fillText(m ? "MUTED" : "SOUND", W - 10, 22);
            ctx.restore();
          }}
        />
      </View>
      <View class="flex flex-row gap-2 items-center">
        <View onPress={flap} activeOpacity={0.7} class="px-4 py-2 rounded-lg bg-sky-500">
          <Text class="text-white font-bold">{phase() === "over" ? "Restart" : "Flap"}</Text>
        </View>
        <View onPress={toggleMute} activeOpacity={0.7} class="px-4 py-2 rounded-lg bg-slate-600">
          <Text class="text-white font-bold">{muted() ? "Sound off" : "Sound on"}</Text>
        </View>
      </View>
      <Text class="text-xs text-slate-500">Space / tap to flap · M to mute</Text>
    </View>
  );
}