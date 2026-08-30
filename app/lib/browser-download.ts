type DownloadAnchor = {
  href: string;
  download: string;
  hidden: boolean;
  rel: string;
  click: () => void;
  remove: () => void;
};

export type BrowserDownloadEnvironment = {
  createObjectUrl: (blob: Blob) => string;
  revokeObjectUrl: (url: string) => void;
  createAnchor: () => DownloadAnchor;
  appendAnchor: (anchor: DownloadAnchor) => void;
  defer: (callback: () => void, delay: number) => void;
};

export const DOWNLOAD_URL_LIFETIME_MS = 30_000;

function browserEnvironment(): BrowserDownloadEnvironment {
  return {
    createObjectUrl: (blob) => URL.createObjectURL(blob),
    revokeObjectUrl: (url) => URL.revokeObjectURL(url),
    createAnchor: () => document.createElement("a"),
    appendAnchor: (anchor) => (document.body ?? document.documentElement).appendChild(anchor as HTMLAnchorElement),
    defer: (callback, delay) => window.setTimeout(callback, delay),
  };
}

export function downloadBytes(
  bytes: Uint8Array,
  name: string,
  type: string,
  environment = browserEnvironment(),
) {
  const blob = new Blob([bytes as BlobPart], { type });
  const url = environment.createObjectUrl(blob);
  const anchor = environment.createAnchor();
  anchor.href = url;
  anchor.download = name;
  anchor.hidden = true;
  anchor.rel = "noopener";
  environment.appendAnchor(anchor);

  try {
    anchor.click();
  } catch (error) {
    anchor.remove();
    environment.revokeObjectUrl(url);
    throw error;
  }

  anchor.remove();
  // WebKit and embedded browser shells can defer consuming a large object URL
  // well beyond the click task. Retain it long enough for the browser download
  // service to take ownership, then release it deterministically.
  environment.defer(() => environment.revokeObjectUrl(url), DOWNLOAD_URL_LIFETIME_MS);
}
