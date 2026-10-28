// The fixture's card: opens full screen or inline (480×320).
import { Text, View } from "nsh/base";
import { rox } from "rox";

export default function Card() {
  return (
    <View>
      <Text>{String(rox.props.label ?? "Fixture card")}</Text>
    </View>
  );
}
