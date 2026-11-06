// Voice input for the search boxes (src-tauri/src/speech.rs).
//
// Order of attempts:
//   1. the system recognizer when the OS has one and we are online (Android: Google – Hebrew, its
//      own "speak now" window, works offline too with Google's Hebrew pack);
//   2. the offline ivrit-ai Whisper model from "Data": the webview records the utterance here
//      (16 kHz mono, stops after a pause or a second press) and the backend turns it into text.
import { invoke } from "@tauri-apps/api/core";

export interface SpeechStatus {
  installed: boolean;
  sizeBytes: number;
  downloadBytes: number;
  busy: boolean;
  systemAvailable: boolean;
}

export const speechStatus = () => invoke<SpeechStatus>("speech_status");
export const speechInstall = () => invoke<void>("speech_install");
export const speechCancel = () => invoke<void>("speech_cancel");
export const speechDelete = () => invoke<void>("speech_delete");

/** Progress of the model download (event `speech-progress`). */
export interface SpeechProgress {
  phase: string;
  done: number;
  total: number;
  message: string;
}

export type MicPhase = "idle" | "listening" | "working";

/** A running voice input: `stop()` ends the recording early (second press on the microphone). */
export interface Listening {
  stop: () => void;
}

const RATE = 16000;
/** Silence after speech that ends the recording. */
const END_SILENCE_MS = 1200;
/** No speech at all within this time: give up. */
const NO_SPEECH_MS = 7000;
const MAX_MS = 15000;

/** Records one utterance; resolves with 16-bit PCM (16 kHz mono), empty when nothing was said. */
function recordUtterance(handle: { stopped: boolean }, onLevel?: (v: number) => void): Promise<Int16Array> {
  return new Promise((resolve, reject) => {
    void (async () => {
      let stream: MediaStream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true } });
      } catch (e) {
        reject({ code: "MIC_DENIED", message: String((e as Error)?.message ?? e) });
        return;
      }
      const ctx = new AudioContext({ sampleRate: RATE });
      const src = ctx.createMediaStreamSource(stream);
      // ScriptProcessor: no worklet module to load (CSP), plenty for a few seconds of speech
      const proc = ctx.createScriptProcessor(4096, 1, 1);
      const mute = ctx.createGain();
      mute.gain.value = 0;
      const chunks: Float32Array[] = [];
      const t0 = performance.now();
      let noise = 0.004;
      let heard = false;
      let lastLoud = t0;
      const finish = () => {
        proc.onaudioprocess = null;
        try {
          src.disconnect();
          proc.disconnect();
        } catch {
          /* already disconnected */
        }
        stream.getTracks().forEach((t) => t.stop());
        void ctx.close();
        const n = chunks.reduce((a, c) => a + c.length, 0);
        const out = new Int16Array(heard ? n : 0);
        if (heard) {
          let o = 0;
          for (const c of chunks) for (let i = 0; i < c.length; i++) out[o++] = Math.max(-1, Math.min(1, c[i])) * 32767;
        }
        resolve(out);
      };
      proc.onaudioprocess = (ev) => {
        const data = new Float32Array(ev.inputBuffer.getChannelData(0));
        chunks.push(data);
        let sum = 0;
        for (let i = 0; i < data.length; i++) sum += data[i] * data[i];
        const rms = Math.sqrt(sum / data.length);
        const now = performance.now();
        // the first half second sets the noise floor; speech is clearly above it
        if (now - t0 < 500) noise = Math.max(noise, rms);
        const loud = rms > Math.max(0.015, noise * 2.5);
        if (loud) {
          heard = true;
          lastLoud = now;
        }
        onLevel?.(Math.min(1, rms * 8));
        const elapsed = now - t0;
        if (handle.stopped || elapsed > MAX_MS || (heard && now - lastLoud > END_SILENCE_MS) || (!heard && elapsed > NO_SPEECH_MS)) finish();
      };
      src.connect(proc);
      proc.connect(mute);
      mute.connect(ctx.destination);
    })();
  });
}

/** Bytes → Base64 (the PCM goes as JSON text: Android's webview cannot send raw bodies). */
function toBase64(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  return btoa(s);
}

/**
 * Turns one spoken phrase into text. `onPhase` follows the steps for the microphone icon.
 * Resolves with "" when nothing was said or the user cancelled. Errors: SPEECH_NO_MODEL (no
 * system recognizer and no offline model), MIC_DENIED, others from the backend.
 */
export function listen(onPhase: (p: MicPhase) => void, onLevel?: (v: number) => void): { done: Promise<string> } & Listening {
  const handle = { stopped: false };
  const done = (async () => {
    const st = await speechStatus();
    if (st.systemAvailable && (navigator.onLine || !st.installed)) {
      onPhase("listening");
      try {
        return await invoke<string>("speech_system", { lang: "he-IL" });
      } catch (e) {
        // the system recognizer failed: the offline model is the fallback when there is one
        if (!st.installed) throw e;
      }
    }
    if (!st.installed) throw { code: "SPEECH_NO_MODEL", message: "SPEECH_NO_MODEL" };
    onPhase("listening");
    const pcm = await recordUtterance(handle, onLevel);
    if (!pcm.length) return "";
    onPhase("working");
    return await invoke<string>("speech_transcribe", { pcm: toBase64(new Uint8Array(pcm.buffer)) });
  })().finally(() => onPhase("idle"));
  return { done, stop: () => (handle.stopped = true) };
}
