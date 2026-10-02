import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { commands, type AppleAsrMode } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";

export function AppleAccelerationSelector() {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [available, setAvailable] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const mode = getSetting("apple_asr_mode") ?? "off";
  const busy =
    isUpdating("apple_asr_mode") ||
    isUpdating("apple_asr_prewarm") ||
    isUpdating("apple_asr_keep_loaded");
  const key = "settings.advanced.acceleration.apple";
  useEffect(() => {
    let disposed = false;
    commands.getAvailableAccelerators().then((options) => {
      if (!disposed) setAvailable(options.apple_native);
    });
    const unlisten = listen<{ downloaded_bytes: number; total_bytes: number }>(
      "apple-asr-download-progress",
      (event) => {
        setProgress(
          Math.round(
            (100 * event.payload.downloaded_bytes) /
              Math.max(1, event.payload.total_bytes),
          ),
        );
      },
    );
    const fallback = listen<string>("apple-asr-fallback", () =>
      toast.warning(t(`${key}.fallback`)),
    );
    return () => {
      disposed = true;
      void unlisten.then((stop) => stop());
      void fallback.then((stop) => stop());
    };
  }, [t]);
  useEffect(() => {
    if (!busy) setProgress(null);
  }, [busy]);
  if (!available) return null;
  return (
    <>
      <SettingContainer
        title={t(`${key}.title`)}
        description={t(`${key}.description`)}
        descriptionMode="inline"
        grouped
        layout="horizontal"
      >
        <Dropdown
          options={[
            { value: "off", label: t(`${key}.off`) },
            { value: "gpu", label: t(`${key}.gpu`) },
            { value: "neural", label: t(`${key}.neural`) },
          ]}
          selectedValue={mode}
          onSelect={(value) =>
            updateSetting("apple_asr_mode", value as AppleAsrMode)
          }
          disabled={busy}
        />
      </SettingContainer>
      {busy && (
        <div
          className="flex items-center justify-between p-3 text-sm"
          role="status"
        >
          <span>
            {progress === null
              ? t(`${key}.preparing`)
              : t(`${key}.progress`, { percent: progress })}
          </span>
          <button
            type="button"
            onClick={() => commands.cancelAppleAsrDownload()}
          >
            {t(`${key}.cancel`)}
          </button>
        </div>
      )}
      {mode !== "off" && (
        <>
          <ToggleSwitch
            checked={getSetting("apple_asr_prewarm") ?? true}
            onChange={(value) => updateSetting("apple_asr_prewarm", value)}
            disabled={busy}
            label={t(`${key}.prewarmTitle`)}
            description={t(`${key}.prewarmDescription`)}
            descriptionMode="inline"
            grouped
          />
          <ToggleSwitch
            checked={getSetting("apple_asr_keep_loaded") ?? false}
            onChange={(value) => updateSetting("apple_asr_keep_loaded", value)}
            disabled={busy}
            label={t(`${key}.keepTitle`)}
            description={t(`${key}.keepDescription`)}
            descriptionMode="inline"
            grouped
          />
        </>
      )}
    </>
  );
}
