import { describe, expect, it } from "vitest";

import { darkTheme, lightTheme } from "./theme";

describe.each([
  ["light", lightTheme],
  ["dark", darkTheme],
] as const)("%s theme contrast", (_name, theme) => {
  it.each([
    ["primary text", theme.colorNeutralForeground1, theme.colorNeutralBackground2],
    ["secondary text", theme.colorNeutralForeground2, theme.colorNeutralBackground2],
    ["brand controls", theme.colorNeutralForegroundOnBrand, theme.colorBrandBackground],
  ])("keeps %s at WCAG AA normal-text contrast", (_case, foreground, background) => {
    expect(contrastRatio(foreground, background)).toBeGreaterThanOrEqual(4.5);
  });
});

function contrastRatio(foreground: string, background: string): number {
  const foregroundLuminance = relativeLuminance(foreground);
  const backgroundLuminance = relativeLuminance(background);
  const lighter = Math.max(foregroundLuminance, backgroundLuminance);
  const darker = Math.min(foregroundLuminance, backgroundLuminance);
  return (lighter + 0.05) / (darker + 0.05);
}

function relativeLuminance(hex: string): number {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)
    ?.map((channel) => Number.parseInt(channel, 16) / 255);
  if (!channels || channels.length !== 3) throw new Error(`expected six-digit hex color: ${hex}`);
  const [red, green, blue] = channels.map((channel) =>
    channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}
