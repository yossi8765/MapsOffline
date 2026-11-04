import { CircleAlert } from "lucide-react";
import { Icon } from "./Icon";
import { Tip } from "./Tip";

/** A small "!" whose explanation appears on hover / keyboard focus: long hints stay out of the
 *  panels (clean look) but remain one pointer move away. */
export function InfoTip({ text, label }: { text: string; label?: string }) {
  return (
    <Tip text={text}>
      <span
        role="img"
        tabIndex={0}
        aria-label={label ?? text}
        className="inline-flex items-center justify-center text-subtle hover:text-fg focus-visible:text-fg cursor-help shrink-0"
        onClick={(e) => e.preventDefault()}
      >
        <Icon as={CircleAlert} size={15} />
      </span>
    </Tip>
  );
}
