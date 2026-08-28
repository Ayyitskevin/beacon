import {
  type BrandVariants,
  createDarkTheme,
  createLightTheme,
} from "@fluentui/react-components";

const institutionalBrand: BrandVariants = {
  10: "#0d1924",
  20: "#14283a",
  30: "#1a374f",
  40: "#204665",
  50: "#27557b",
  60: "#2c5a85",
  70: "#2f5d89",
  80: "#315f8c",
  90: "#4a769f",
  100: "#638cb1",
  110: "#7da2c2",
  120: "#97b7d2",
  130: "#b2cbe0",
  140: "#cdddeb",
  150: "#e1ebf3",
  160: "#f1f6fa",
};

export const lightTheme = {
  ...createLightTheme(institutionalBrand),
  colorNeutralBackground1: "#f7f8fa",
  colorNeutralBackground2: "#f1f3f5",
  colorNeutralForeground1: "#172033",
  colorNeutralForeground2: "#5b6474",
  colorNeutralStroke2: "#dfe3e8",
};

export const darkTheme = {
  ...createDarkTheme(institutionalBrand),
  colorNeutralBackground1: "#11151c",
  colorNeutralBackground2: "#171c25",
  colorNeutralForeground1: "#f3f5f7",
  colorNeutralForeground2: "#b9c0ca",
  colorNeutralStroke2: "#2d3541",
};
