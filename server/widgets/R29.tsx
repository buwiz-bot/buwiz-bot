import { signal, Text, View } from "nsh";

export default function R() {
  const display = signal("0");
  let acc: number | null = null;
  let op: string | null = null;
  let fresh = false;

  const calc = (a: number, b: number, o: string) => {
    if (o === "+") return a + b;
    if (o === "-") return a - b;
    if (o === "×") return a * b;
    return a / b;
  };

  const fmt = (n: number) => (isFinite(n) ? String(Number(n.toPrecision(12))) : "Error");

  const digit = (d: string) => {
    if (fresh) {
      display.set(d);
      fresh = false;
      return;
    }
    const cur: string = display();
    display.set(cur === "0" ? d : cur + d);
  };

  const dot = () => {
    if (fresh) {
      display.set("0.");
      fresh = false;
      return;
    }
    const cur: string = display();
    if (cur.indexOf(".") < 0) display.set(cur + ".");
  };

  const pressOp = (o: string) => {
    const cur = parseFloat(display());
    if (acc !== null && op && !fresh) {
      acc = calc(acc, cur, op);
      display.set(fmt(acc));
    } else {
      acc = cur;
    }
    op = o;
    fresh = true;
  };

  const equals = () => {
    if (acc === null || !op) return;
    const r = calc(acc, parseFloat(display()), op);
    display.set(fmt(r));
    acc = null;
    op = null;
    fresh = true;
  };

  const clear = () => {
    display.set("0");
    acc = null;
    op = null;
    fresh = false;
  };

  const backspace = () => {
    if (fresh) return;
    const cur: string = display();
    const s = cur.substring(0, cur.length - 1);
    display.set(s === "" || s === "-" ? "0" : s);
  };

  const key = (label: string, onPress: () => void, cls: string) => (
    <View onPress={onPress} activeOpacity={0.7} class={cls}>
      <Text class="text-xl font-semibold text-white">{label}</Text>
    </View>
  );

  const NUM = "flex-1 items-center justify-center rounded-xl py-4 bg-zinc-700";
  const OP = "flex-1 items-center justify-center rounded-xl py-4 bg-orange-500";
  const FN = "flex-1 items-center justify-center rounded-xl py-4 bg-zinc-500";

  return (
    <View class="flex flex-col gap-3 p-4 rounded-2xl bg-zinc-900 w-full">
      <View class="items-end justify-center px-2 py-4 min-h-20">
        <Text class="text-4xl font-light text-white">{display()}</Text>
      </View>

      <View class="flex flex-row gap-2">
        {key("C", clear, FN)}
        {key("⌫", backspace, FN)}
        {key("%", () => display.set(fmt(parseFloat(display()) / 100)), FN)}
        {key("÷", () => pressOp("÷"), OP)}
      </View>

      <View class="flex flex-row gap-2">
        {key("7", () => digit("7"), NUM)}
        {key("8", () => digit("8"), NUM)}
        {key("9", () => digit("9"), NUM)}
        {key("×", () => pressOp("×"), OP)}
      </View>

      <View class="flex flex-row gap-2">
        {key("4", () => digit("4"), NUM)}
        {key("5", () => digit("5"), NUM)}
        {key("6", () => digit("6"), NUM)}
        {key("-", () => pressOp("-"), OP)}
      </View>

      <View class="flex flex-row gap-2">
        {key("1", () => digit("1"), NUM)}
        {key("2", () => digit("2"), NUM)}
        {key("3", () => digit("3"), NUM)}
        {key("+", () => pressOp("+"), OP)}
      </View>

      <View class="flex flex-row gap-2">
        {key("0", () => digit("0"), NUM)}
        {key(".", dot, NUM)}
        {key("=", equals, OP)}
      </View>
    </View>
  );
}