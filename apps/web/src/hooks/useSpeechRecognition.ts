import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Thin React wrapper around the Web Speech API (`SpeechRecognition` /
 * `webkitSpeechRecognition`). WebView2 on Windows 11 ships Chromium's impl
 * which routes to an online service, so this won't work offline — callers
 * surface that state via `supported`/`error`.
 */

type RecognitionCtor = new () => RawRecognition;

interface RawRecognition extends EventTarget {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  start(): void;
  stop(): void;
  abort(): void;
  onresult: ((e: RawRecognitionEvent) => void) | null;
  onerror: ((e: Event & { error: string }) => void) | null;
  onend: (() => void) | null;
}

interface RawRecognitionEvent extends Event {
  results: RawResultList;
  resultIndex: number;
}

interface RawResultList {
  length: number;
  [i: number]: RawResult;
}

interface RawResult {
  isFinal: boolean;
  [i: number]: RawAlt;
}

interface RawAlt {
  transcript: string;
  confidence: number;
}

function getCtor(): RecognitionCtor | null {
  const w = window as unknown as {
    SpeechRecognition?: RecognitionCtor;
    webkitSpeechRecognition?: RecognitionCtor;
  };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

/** Normalize our i18n locale keys to the BCP-47 form Speech API expects. */
export function toSpeechLocale(locale: string | undefined): string {
  if (!locale) return "en-US";
  if (locale.includes("-")) return locale;
  switch (locale) {
    case "en":
      return "en-US";
    case "pt":
      return "pt-BR";
    default:
      return locale;
  }
}

/**
 * Spoken end-of-message command. Saying "câmbio" (radio "over") at the end of a
 * dictation submits the message. Matches the trailing word case-insensitively,
 * with or without the accent, and tolerates trailing punctuation the recognizer
 * may append (e.g. "câmbio.").
 */
const VOICE_SUBMIT_RE = /[\s,.!?]*\bc[âa]mbio\b[\s,.!?]*$/iu;

/**
 * Detect the trailing "câmbio" submit command in a transcript chunk. Returns the
 * chunk with the command stripped and whether a submit was requested.
 */
export function stripVoiceSubmitCommand(text: string): {
  text: string;
  submit: boolean;
} {
  if (VOICE_SUBMIT_RE.test(text)) {
    return { text: text.replace(VOICE_SUBMIT_RE, ""), submit: true };
  }
  return { text, submit: false };
}

export interface SpeechRecognitionHook {
  supported: boolean;
  listening: boolean;
  interim: string;
  error: string | null;
  start: () => void;
  stop: () => void;
  toggle: () => void;
}

export function useSpeechRecognition({
  lang,
  onFinal,
}: {
  lang: string;
  onFinal: (text: string) => void;
}): SpeechRecognitionHook {
  const [listening, setListening] = useState(false);
  const [interim, setInterim] = useState("");
  const [error, setError] = useState<string | null>(null);
  const recRef = useRef<RawRecognition | null>(null);
  // Keep the latest onFinal without re-creating start() every render.
  const onFinalRef = useRef(onFinal);
  useEffect(() => {
    onFinalRef.current = onFinal;
  }, [onFinal]);

  const Ctor = getCtor();
  const supported = !!Ctor;

  const start = useCallback(() => {
    if (!Ctor) {
      setError("no_support");
      return;
    }
    if (recRef.current) return;
    setError(null);
    setInterim("");
    const rec = new Ctor();
    rec.lang = lang;
    rec.continuous = true;
    rec.interimResults = true;
    rec.onresult = (e) => {
      let interimText = "";
      for (let i = e.resultIndex; i < e.results.length; i += 1) {
        const result = e.results[i];
        if (!result) continue;
        const alt = result[0];
        const transcript = alt?.transcript ?? "";
        if (result.isFinal) {
          onFinalRef.current(transcript);
        } else {
          interimText += transcript;
        }
      }
      setInterim(interimText);
    };
    rec.onerror = (e) => {
      setError(e.error || "unknown");
    };
    rec.onend = () => {
      recRef.current = null;
      setListening(false);
      setInterim("");
    };
    try {
      rec.start();
      recRef.current = rec;
      setListening(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [Ctor, lang]);

  const stop = useCallback(() => {
    recRef.current?.stop();
  }, []);

  const toggle = useCallback(() => {
    if (recRef.current) recRef.current.stop();
    else start();
  }, [start]);

  // Abort on unmount — `stop()` waits for a final result which can leak.
  useEffect(() => {
    return () => {
      recRef.current?.abort();
      recRef.current = null;
    };
  }, []);

  return { supported, listening, interim, error, start, stop, toggle };
}
