// The template's app: greets the friend in buwiz.props.ticket, shows where
// it is mounted. Imports only nsh, nsh/base and buwiz (spec 6.3); data comes
// from buwiz.call alone (no fetch, no network).
import { signal, Text, View } from "nsh";
import { buwiz } from "buwiz";

export default function Hello() {
  const greeting = signal("Saying hello…");
  const ticket = buwiz.props.ticket;

  buwiz.call("hello_greet", ticket ? { ticket } : {}).then(
    (r) => greeting.set(String(r.greeting)),
    (e) => greeting.set(`Could not say hello (${e.code})`),
  );

  // Buwiz Bot renders the app in its own theme (spec 6.5): every color has its
  // dark: variant, so the app follows Buwiz Bot's light or dark setting
  return (
    <View class="flex-1 gap-2 p-4 bg-white dark:bg-[#161616]">
      <Text class="text-[14px] text-[#1c1c1c] dark:text-[#f5f5f5]">{greeting()}</Text>
      <Text class="text-[12px] text-[#1c1c1c99] dark:text-[#f5f5f599]">{`${buwiz.viewport().placement}: ${buwiz.viewport().width}×${buwiz.viewport().height}`}</Text>
    </View>
  );
}
