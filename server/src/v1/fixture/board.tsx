// The fixture's full-screen app: every bridge method the manifest grants.
import { signal, Text, View } from "nsh";
import { buwiz } from "buwiz";

export default function Board() {
  const echoed = signal("");

  buwiz.call("fx_echo", { from: "board" }).then((r) => echoed.set(JSON.stringify(r)));

  return (
    <View>
      <Text>Fixture board</Text>
      <Text>{echoed()}</Text>
    </View>
  );
}
