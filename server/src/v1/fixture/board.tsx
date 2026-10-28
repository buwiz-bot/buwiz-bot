// The fixture's full-screen app: every bridge method the manifest grants.
import { signal } from "nsh";
import { Text, View } from "nsh/base";
import { rox } from "rox";

export default function Board() {
  const echoed = signal("");

  rox.call("fx_echo", { from: "board" }).then((r) => echoed.set(JSON.stringify(r)));

  return (
    <View>
      <Text>Fixture board</Text>
      <Text>{echoed()}</Text>
    </View>
  );
}
