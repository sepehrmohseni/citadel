import cascadiaCode from "@fontsource-variable/cascadia-code/files/cascadia-code-latin-wght-normal.woff2?url";
import cascadiaCodeExt from "@fontsource-variable/cascadia-code/files/cascadia-code-latin-ext-wght-normal.woff2?url";
import cascadiaMono from "@fontsource-variable/cascadia-mono/files/cascadia-mono-latin-wght-normal.woff2?url";
import cascadiaMonoExt from "@fontsource-variable/cascadia-mono/files/cascadia-mono-latin-ext-wght-normal.woff2?url";
import firaCode from "@fontsource-variable/fira-code/files/fira-code-latin-wght-normal.woff2?url";
import firaCodeExt from "@fontsource-variable/fira-code/files/fira-code-latin-ext-wght-normal.woff2?url";
import inconsolata from "@fontsource-variable/inconsolata/files/inconsolata-latin-wght-normal.woff2?url";
import inconsolataExt from "@fontsource-variable/inconsolata/files/inconsolata-latin-ext-wght-normal.woff2?url";
import notoMono from "@fontsource-variable/noto-sans-mono/files/noto-sans-mono-latin-wght-normal.woff2?url";
import notoMonoExt from "@fontsource-variable/noto-sans-mono/files/noto-sans-mono-latin-ext-wght-normal.woff2?url";
import robotoMono from "@fontsource-variable/roboto-mono/files/roboto-mono-latin-wght-normal.woff2?url";
import robotoMonoExt from "@fontsource-variable/roboto-mono/files/roboto-mono-latin-ext-wght-normal.woff2?url";
import sourceCode from "@fontsource-variable/source-code-pro/files/source-code-pro-latin-wght-normal.woff2?url";
import sourceCodeExt from "@fontsource-variable/source-code-pro/files/source-code-pro-latin-ext-wght-normal.woff2?url";
import plex400 from "@fontsource/ibm-plex-mono/files/ibm-plex-mono-latin-400-normal.woff2?url";
import plex400Ext from "@fontsource/ibm-plex-mono/files/ibm-plex-mono-latin-ext-400-normal.woff2?url";
import plex700 from "@fontsource/ibm-plex-mono/files/ibm-plex-mono-latin-700-normal.woff2?url";
import plex700Ext from "@fontsource/ibm-plex-mono/files/ibm-plex-mono-latin-ext-700-normal.woff2?url";
import ubuntu400 from "@fontsource/ubuntu-mono/files/ubuntu-mono-latin-400-normal.woff2?url";
import ubuntu400Ext from "@fontsource/ubuntu-mono/files/ubuntu-mono-latin-ext-400-normal.woff2?url";
import ubuntu700 from "@fontsource/ubuntu-mono/files/ubuntu-mono-latin-700-normal.woff2?url";
import ubuntu700Ext from "@fontsource/ubuntu-mono/files/ubuntu-mono-latin-ext-700-normal.woff2?url";

export const DEFAULT_FONT = "JetBrains Mono Variable";

const LATIN = "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD";
const LATIN_EXT = "U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF";

type Face = [url: string, range: string, weight: string];

const variable = (latin: string, ext: string, weight: string): Face[] => [
  [latin, LATIN, weight],
  [ext, LATIN_EXT, weight],
];

const FACES: Record<string, Face[]> = {
  "Cascadia Code": variable(cascadiaCode, cascadiaCodeExt, "200 700"),
  "Cascadia Mono": variable(cascadiaMono, cascadiaMonoExt, "200 700"),
  "Fira Code": variable(firaCode, firaCodeExt, "300 700"),
  "IBM Plex Mono": [
    [plex400, LATIN, "400"],
    [plex400Ext, LATIN_EXT, "400"],
    [plex700, LATIN, "700"],
    [plex700Ext, LATIN_EXT, "700"],
  ],
  Inconsolata: variable(inconsolata, inconsolataExt, "200 900"),
  "Noto Sans Mono": variable(notoMono, notoMonoExt, "100 900"),
  "Roboto Mono": variable(robotoMono, robotoMonoExt, "100 700"),
  "Source Code Pro": variable(sourceCode, sourceCodeExt, "200 900"),
  "Ubuntu Mono": [
    [ubuntu400, LATIN, "400"],
    [ubuntu400Ext, LATIN_EXT, "400"],
    [ubuntu700, LATIN, "700"],
    [ubuntu700Ext, LATIN_EXT, "700"],
  ],
};

export const BUNDLED_FONTS: string[] = [DEFAULT_FONT, ...Object.keys(FACES)];

let registered = false;

function register(): void {
  if (registered || !document.fonts) return;
  registered = true;
  for (const [family, faces] of Object.entries(FACES)) {
    for (const [url, unicodeRange, weight] of faces) {
      document.fonts.add(new FontFace(family, `url(${url}) format("woff2")`, { unicodeRange, weight, style: "normal", display: "swap" }));
    }
  }
}

register();

export function loadFont(family: string): Promise<unknown> {
  if (!document.fonts) return Promise.resolve();
  const quoted = `"${family.replace(/"/g, "")}"`;
  return Promise.race([
    Promise.all([document.fonts.load(`14px ${quoted}`), document.fonts.load(`bold 14px ${quoted}`)]).catch(() => undefined),
    new Promise((r) => setTimeout(r, 3000)),
  ]);
}
