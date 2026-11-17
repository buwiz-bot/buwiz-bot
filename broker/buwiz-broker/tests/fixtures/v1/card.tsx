// The fixture's card: opens full screen or inline (480×320).
import { Text, View } from "nsh";
import { buwiz } from "buwiz";

export default function Card() {
  return (
    <View>
      <Text>{String(buwiz.props.label ?? "Fixture card")}</Text>
    </View>
  );
}
