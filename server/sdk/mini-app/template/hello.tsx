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

  // Rox renders the app in its own theme (spec 6.5): every color has its
  // dark: variant, so the app follows Rox's light or dark setting
  return (
    <View class="flex-1 gap-2 p-4 bg-white dark:bg-[#161616]">
      <Text class="text-[14px] text-[#1c1c1c] dark:text-[#f5f5f5]">{greeting()}</Text>
      <Text class="text-[12px] text-[#1c1c1c99] dark:text-[#f5f5f599]">{`${rox.viewport().placement}: ${rox.viewport().width}×${rox.viewport().height}`}</Text>
    </View>
  );
}
