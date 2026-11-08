import { useRef, useState } from "react";
import { LoaderCircle, Mic } from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { useT } from "../lib/i18n";
import { useStore } from "../lib/store";
import { errorCode } from "../lib/api";
import { listen, type Listening, type MicPhase } from "../lib/speech";

/**
 * Voice input inside a search box: a bare microphone line icon (no button frame). Press = speak,
 * press again while listening = stop early. The text goes to `onText` (the box searches it).
 */
export function MicButton({ onText, className }: { onText: (text: string) => void; className?: string }) {
  const t = useT();
  const [phase, setPhase] = useState<MicPhase>("idle");
  const [level, setLevel] = useState(0);
  const run = useRef<Listening | null>(null);

  const press = async () => {
    if (phase === "listening") {
      run.current?.stop();
      return;
    }
    if (phase === "working") return;
    const r = listen((p) => {
      setPhase(p);
      // offline recognition takes a few seconds (longer on a slow phone): say what is happening
      if (p === "working") useStore.getState().toast("info", t("speechWorking"), { duration: 4000 });
    }, setLevel);
    run.current = r;
    try {
      const text = (await r.done).trim();
      if (text) onText(text);
      else useStore.getState().toast("info", t("speechNothing"));
    } catch (e) {
      const st = useStore.getState();
      const code = errorCode(e);
      if (code === "SPEECH_NO_MODEL")
        st.toast("info", t("speechNoModel"), { duration: 0, action: { label: t("speechGetModel"), run: () => st.setPanel("data") } });
      else if (code === "MIC_DENIED") st.toast("error", t("speechMicDenied"));
      else if (code === "SPEECH_CPU_UNSUPPORTED") st.toast("error", t("speechCpu"));
      else st.toastError(e);
    } finally {
      run.current = null;
      setLevel(0);
    }
  };

  const label = phase === "listening" ? t("speechStop") : t("speechStart");
  return (
    <button
      type="button"
      // keep the focus (and the open suggestion list) in the input
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => void press()}
      aria-label={label}
      title={label}
      aria-pressed={phase !== "idle"}
      className={clsx(
        "w-7 h-7 flex items-center justify-center bg-transparent border-0 p-0 transition-colors duration-150",
        phase === "idle" ? "text-subtle hover:text-fg" : "text-accent",
        className,
      )}
    >
      {phase === "working" ? (
        <Icon as={LoaderCircle} size={17} className="animate-spin" />
      ) : (
        <span className="relative flex items-center justify-center">
          {phase === "listening" && (
            <span
              aria-hidden
              className="absolute rounded-full bg-accent/20 transition-transform duration-100"
              style={{ width: 26, height: 26, transform: `scale(${0.6 + level * 0.8})` }}
            />
          )}
          <Icon as={Mic} size={17} strokeWidth={1.75} className="relative" />
        </span>
      )}
    </button>
  );
}
