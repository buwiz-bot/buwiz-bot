// The template's app: greets the friend in rox.props.ticket, shows where
// it is mounted. Imports only nsh, nsh/base and rox (spec 6.3); data comes
// from rox.call alone (no fetch, no network).
import { signal } from "nsh";
import { Text, View } from "nsh/base";
import { rox } from "rox";

export default function Hello() {
  const greeting = signal("Saying hello…");
  const ticket = rox.props.ticket;

  rox.call("hello_greet", ticket ? { ticket } : {}).then(
    (r) => greeting.set(String(r.greeting)),
    (e) => greeting.set(`Could not say hello (${e.code})`),
  );

  return (
    <View>
      <Text>{greeting()}</Text>
      <Text>{`${rox.viewport().placement}: ${rox.viewport().width}×${rox.viewport().height}`}</Text>
    </View>
  );
}
