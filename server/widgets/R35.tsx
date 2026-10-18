import { signal, effect, cleanup, Text, View } from "nsh";
import { Canvas } from "nsh/canvas";

const COLORS: string[] = ['#22d3ee', '#facc15', '#c084fc', '#4ade80', '#f87171', '#60a5fa', '#fb923c'];
const SHAPES: number[][][] = [
  [[0,1],[1,1],[2,1],[3,1]],
  [[1,1],[2,1],[1,2],[2,2]],
  [[1,0],[0,1],[1,1],[2,1]],
  [[1,0],[2,0],[0,1],[1,1]],
  [[0,0],[1,0],[1,1],[2,1]],
  [[0,0],[0,1],[1,1],[2,1]],
  [[2,0],[0,1],[1,1],[2,1]],
];

function shape(t: number, r: number): number[][] {
  let c: number[][] = SHAPES[t];
  for (let i = 0; i < r; i++) c = c.map(([x, y]: number[]) => [3 - y, x]);
  return c;
}

type Piece = { t: number; r: number; x: number; y: number };

export default function R() {
  const board = signal<number[]>(Array(200).fill(0));
  const piece = signal<Piece>({ t: 0, r: 0, x: 3, y: 0 });
  const score = signal(0);
  const lines = signal(0);
  const over = signal(false);
  const level = () => Math.floor(lines() / 10) + 1;

  function hit(t: number, r: number, x: number, y: number, b: number[] = board()): boolean {
    return shape(t, r).some(([cx, cy]: number[]) => {
      const nx = x + cx, ny = y + cy;
      return nx < 0 || nx > 9 || ny > 19 || (ny >= 0 && !!b[ny * 10 + nx]);
    });
  }

  function spawn() {
    const t = Math.floor(Math.random() * 7);
    if (hit(t, 0, 3, 0)) over.set(true);
    piece.set({ t, r: 0, x: 3, y: 0 });
  }

  function lock(p: Piece) {
    const b = board().slice();
    shape(p.t, p.r).forEach(([cx, cy]: number[]) => {
      const ny = p.y + cy;
      if (ny >= 0) b[ny * 10 + p.x + cx] = p.t + 1;
    });
    const kept: number[] = [];
    for (let r = 0; r < 20; r++) {
      const row = b.slice(r * 10, r * 10 + 10);
      if (!row.every(v => v)) kept.push(...row);
    }
    const cleared = 20 - kept.length / 10;
    board.set(Array(cleared * 10).fill(0).concat(kept));
    if (cleared) {
      score.update(s => s + [0, 100, 300, 500, 800][cleared]);
      lines.update(n => n + cleared);
    }
    score.update(s => s + 1);
    spawn();
  }

  function step() {
    if (over()) return;
    const p = piece();
    if (hit(p.t, p.r, p.x, p.y + 1)) lock(p);
    else piece.set({ ...p, y: p.y + 1 });
  }

  function move(dx: number) {
    const p = piece();
    if (!over() && !hit(p.t, p.r, p.x + dx, p.y)) piece.set({ ...p, x: p.x + dx });
  }

  function rotate() {
    const p = piece();
    if (over()) return;
    const r = (p.r + 1) % 4;
    for (const k of [0, -1, 1, -2, 2]) {
      if (!hit(p.t, r, p.x + k, p.y)) { piece.set({ ...p, r, x: p.x + k }); return; }
    }
  }

  function drop() {
    if (over()) return;
    const p = piece();
    let y = p.y;
    while (!hit(p.t, p.r, p.x, y + 1)) y++;
    lock({ ...p, y });
  }

  function restart() {
    board.set(Array(200).fill(0));
    score.set(0);
    lines.set(0);
    over.set(false);
    spawn();
  }

  function drawCell(ctx: any, x: number, y: number, c: string) {
    ctx.fillStyle = c;
    ctx.fillRect(x * 14 + 1, y * 14 + 1, 12, 12);
  }

  spawn();

  effect(() => {
    const ms = Math.max(100, 600 - (level() - 1) * 50);
    const t = setInterval(step, ms);
    cleanup(() => clearInterval(t));
  });

  return (
    <View class="flex-col items-center gap-3 p-4 bg-gray-900 rounded-2xl">
      <Text class="text-white font-bold text-lg">Tetris</Text>
      <Canvas
        style={{ width: 140, height: 280 }}
        draw={(ctx: any) => {
          const b = board();
          const p = piece();
          ctx.fillStyle = '#111827';
          ctx.fillRect(0, 0, 140, 280);
          for (let i = 0; i < 200; i++) {
            const v = b[i];
            if (v) drawCell(ctx, i % 10, Math.floor(i / 10), COLORS[v - 1]);
          }
          if (!over()) {
            shape(p.t, p.r).forEach(([cx, cy]: number[]) => {
              if (p.y + cy >= 0) drawCell(ctx, p.x + cx, p.y + cy, COLORS[p.t]);
            });
          }
        }}
      />
      <Text class="text-gray-400 text-sm">{`Score ${score()}  Lines ${lines()}  Level ${level()}`}</Text>
      <If when={over()}>
        <View class="flex-col items-center gap-2">
          <Text class="text-red-400 font-bold">Game over</Text>
          <View class="px-4 py-2 rounded-lg bg-indigo-500" onPress={restart} activeOpacity={0.7}>
            <Text class="text-white font-bold">Restart</Text>
          </View>
        </View>
      </If>
      <View class="flex-row gap-2">
        <View class="px-4 py-2 rounded-lg bg-gray-700" onPress={() => move(-1)} activeOpacity={0.7}>
          <Text class="text-white font-bold">Left</Text>
        </View>
        <View class="px-4 py-2 rounded-lg bg-gray-700" onPress={() => move(1)} activeOpacity={0.7}>
          <Text class="text-white font-bold">Right</Text>
        </View>
        <View class="px-4 py-2 rounded-lg bg-cyan-600" onPress={rotate} activeOpacity={0.7}>
          <Text class="text-white font-bold">Rotate</Text>
        </View>
        <View class="px-4 py-2 rounded-lg bg-amber-500" onPress={drop} activeOpacity={0.7}>
          <Text class="text-gray-900 font-bold">Drop</Text>
        </View>
      </View>
    </View>
  );
}