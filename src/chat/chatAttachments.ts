/**
 * Build prompt text from files picked in the chat composer (browser File API).
 */

export interface PickedAttachment {
  id: string;
  name: string;
  kind: "image" | "text" | "other";
  /** Shown in the composer chip row. */
  preview: string;
}

const TEXT_LIKE =
  /\.(txt|md|markdown|json|jsonc|yaml|yml|csv|ts|tsx|js|jsx|py|rs|go|java|html|css|xml|toml|env|log)$/i;

export async function filesToAttachments(files: File[]): Promise<PickedAttachment[]> {
  const out: PickedAttachment[] = [];
  for (const file of files) {
    const id = `${file.name}-${file.size}-${file.lastModified}`;
    if (file.type.startsWith("image/")) {
      out.push({ id, name: file.name, kind: "image", preview: file.name });
    } else if (file.type.startsWith("text/") || TEXT_LIKE.test(file.name)) {
      out.push({ id, name: file.name, kind: "text", preview: file.name });
    } else {
      out.push({ id, name: file.name, kind: "other", preview: `${file.name} (${formatBytes(file.size)})` });
    }
  }
  return out;
}

export async function attachmentsToPrompt(files: File[]): Promise<string> {
  const blocks: string[] = [];
  for (const file of files) {
    if (file.type.startsWith("image/")) {
      const data = await readAsDataUrl(file);
      blocks.push(
        `[Image attached: ${file.name}]\nThe image is included below as a data URL for vision-capable models.\n${data}`,
      );
    } else if (file.type.startsWith("text/") || TEXT_LIKE.test(file.name)) {
      const text = await file.text();
      const clipped = text.length > 48_000 ? `${text.slice(0, 48_000)}\n…(truncated)` : text;
      blocks.push(`[File: ${file.name}]\n\`\`\`\n${clipped}\n\`\`\``);
    } else {
      blocks.push(
        `[Binary attachment: ${file.name}, ${formatBytes(file.size)}. Describe what you need and re-attach as text or a screenshot if the model cannot read it.]`,
      );
    }
  }
  return blocks.join("\n\n");
}

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}
