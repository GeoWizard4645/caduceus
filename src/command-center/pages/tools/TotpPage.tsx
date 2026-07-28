/**
 * 2FA code picker: store a TOTP secret, watch the current 6-digit code.
 *
 * Mirrors `src-tauri/src/tools/totp.rs`. The secret is written to the OS
 * keychain by `totpAddAccount` and never comes back across the IPC boundary
 * afterwards — only the label/issuer/digits/period metadata and the current
 * code do.
 */

import { useEffect, useRef, useState } from "react";

import * as api from "@/shared/api";
import type { TotpAccount, TotpCurrentCode } from "@/shared/api";
import {
  Button,
  Callout,
  EmptyState,
  Field,
  IconButton,
  Section,
  Select,
  Spinner,
  TextInput,
  cx,
} from "@/shared/ui";
import type { ToolPageProps } from "../ToolPage";

export function TotpPage({ onSetTitle }: ToolPageProps) {
  useEffect(() => onSetTitle("2FA Codes"), [onSetTitle]);

  const [accounts, setAccounts] = useState<TotpAccount[]>([]);
  const [codes, setCodes] = useState<Record<string, TotpCurrentCode>>({});
  const [loaded, setLoaded] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);

  const [showAdd, setShowAdd] = useState(false);
  const [label, setLabel] = useState("");
  const [issuer, setIssuer] = useState("");
  const [secret, setSecret] = useState("");
  const [digits, setDigits] = useState("6");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const accountsRef = useRef<TotpAccount[]>([]);
  accountsRef.current = accounts;

  const refreshCodes = async () => {
    const list = accountsRef.current;
    if (list.length === 0) return;
    const entries = await Promise.all(
      list.map(async (a) => {
        try {
          return [a.id, await api.totpCurrentCode(a.id)] as const;
        } catch {
          return null;
        }
      }),
    );
    setCodes((current) => {
      const next = { ...current };
      for (const entry of entries) {
        if (entry) next[entry[0]] = entry[1];
      }
      return next;
    });
  };

  const reload = () => {
    api
      .totpListAccounts()
      .then((list) => {
        setAccounts(list);
        setLoaded(true);
      })
      .catch((e) => setListError(api.errorMessage(e)));
  };

  useEffect(reload, []);

  // Codes rotate every `period` seconds; re-asking the backend once a second
  // is cheap (pure HMAC math, no I/O per call beyond a keychain read) and
  // means the countdown and the code itself can never drift out of sync with
  // each other the way a client-side timer alone could.
  useEffect(() => {
    void refreshCodes();
    const id = window.setInterval(() => void refreshCodes(), 1000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accounts.length]);

  const resetForm = () => {
    setLabel("");
    setIssuer("");
    setSecret("");
    setDigits("6");
    setSaveError(null);
  };

  const add = async () => {
    if (!label.trim()) {
      setSaveError("Give this account a label, like \"Ada @ GitHub\".");
      return;
    }
    if (!secret.trim()) {
      setSaveError("Paste the secret key first.");
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      const account = await api.totpAddAccount(label, issuer || undefined, secret, Number(digits));
      setAccounts((current) => [...current, account]);
      resetForm();
      setShowAdd(false);
    } catch (e) {
      setSaveError(api.errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (account: TotpAccount) => {
    if (deleteArmedId !== account.id) {
      setDeleteArmedId(account.id);
      return;
    }
    setDeleteArmedId(null);
    try {
      await api.totpDeleteAccount(account.id);
      setAccounts((current) => current.filter((a) => a.id !== account.id));
    } catch (e) {
      setListError(api.errorMessage(e));
    }
  };

  const copyCode = (code: string) => {
    void navigator.clipboard.writeText(code);
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-[680px] px-6 py-5">
        <div className="mb-4">
          <h1 className="text-[17px] font-semibold tracking-[-0.015em] text-ink">2FA codes</h1>
          <p className="mt-1 max-w-prose text-[13px] leading-relaxed text-ink-mute">
            Time-based one-time codes, generated on this Mac. Each secret is stored only in the macOS
            keychain — never in a plain settings file, and never sent anywhere.
          </p>
        </div>

        <Section
          title="Accounts"
          actions={
            <Button size="sm" tone="primary" onClick={() => setShowAdd((v) => !v)}>
              {showAdd ? "Cancel" : "+ Add secret"}
            </Button>
          }
        >
          {listError && <p className="mb-3 text-2xs text-danger">{listError}</p>}

          {showAdd && (
            <div className="mb-4 space-y-3 rounded-lg border border-line bg-base/40 p-3">
              <div className="grid grid-cols-2 gap-2">
                <Field label="Label" error={saveError}>
                  <TextInput value={label} onChange={setLabel} placeholder="Ada @ GitHub" />
                </Field>
                <Field label="Issuer" hint="Optional.">
                  <TextInput value={issuer} onChange={setIssuer} placeholder="GitHub" />
                </Field>
              </div>
              <Field label="Secret key" hint="The base32 key shown when the site set up 2FA — not the QR code image.">
                <TextInput value={secret} onChange={setSecret} mono placeholder="JBSWY3DPEHPK3PXP" />
              </Field>
              <Field label="Digits">
                <Select
                  value={digits}
                  onChange={setDigits}
                  options={[
                    { value: "6", label: "6 digits (most common)" },
                    { value: "7", label: "7 digits" },
                    { value: "8", label: "8 digits" },
                  ]}
                />
              </Field>
              <Button tone="primary" onClick={() => void add()} disabled={saving}>
                {saving ? <Spinner /> : null} Save
              </Button>
            </div>
          )}

          {!loaded ? (
            <div className="flex items-center justify-center py-8 text-ink-faint">
              <Spinner />
            </div>
          ) : accounts.length === 0 ? (
            <EmptyState
              title="No accounts yet"
              hint="Add a secret above to see a live code here."
              icon="🔐"
            />
          ) : (
            <div className="space-y-2">
              {accounts.map((account) => {
                const code = codes[account.id];
                const fraction = code ? code.secondsRemaining / code.period : 1;
                return (
                  <div
                    key={account.id}
                    className="flex items-center gap-3 rounded-lg border border-line bg-base/40 p-3"
                  >
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[13px] font-medium text-ink">{account.label}</p>
                      {account.issuer && <p className="truncate text-2xs text-ink-faint">{account.issuer}</p>}
                    </div>
                    <button
                      type="button"
                      title="Copy code"
                      onClick={() => code && copyCode(code.code)}
                      className="font-mono text-lg tracking-[0.15em] text-ink hover:text-accent"
                    >
                      {code ? formatCode(code.code) : "······"}
                    </button>
                    <div className="relative h-6 w-6 shrink-0" title={code ? `${code.secondsRemaining}s left` : ""}>
                      <svg viewBox="0 0 24 24" className="h-6 w-6 -rotate-90">
                        <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" strokeWidth="2.5" className="text-line" />
                        <circle
                          cx="12"
                          cy="12"
                          r="10"
                          fill="none"
                          stroke="currentColor"
                          strokeWidth="2.5"
                          strokeDasharray={`${2 * Math.PI * 10}`}
                          strokeDashoffset={`${2 * Math.PI * 10 * (1 - fraction)}`}
                          className={cx("transition-[stroke-dashoffset] duration-1000 ease-linear", fraction < 0.2 ? "text-danger" : "text-accent")}
                        />
                      </svg>
                    </div>
                    <IconButton
                      label={deleteArmedId === account.id ? "Confirm delete" : "Remove account"}
                      tone="danger"
                      onClick={() => void remove(account)}
                    >
                      {deleteArmedId === account.id ? "!" : "×"}
                    </IconButton>
                  </div>
                );
              })}
            </div>
          )}
        </Section>

        <Callout tone="info">
          Removing an account deletes its secret from the keychain immediately — there is no way to
          recover it here afterwards. Keep whatever backup codes the site gave you.
        </Callout>
      </div>
    </div>
  );
}

/** "123456" -> "123 456" — easier to read and to type back in. */
function formatCode(code: string): string {
  if (code.length <= 4) return code;
  const mid = Math.ceil(code.length / 2);
  return `${code.slice(0, mid)} ${code.slice(mid)}`;
}
