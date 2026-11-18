import { ImageResponse } from "next/og";

import { Mark } from "@/components/brand";

export const size = { width: 180, height: 180 };
export const contentType = "image/png";

/** The icon a phone puts on its home screen, and iMessage beside a link: `icon.svg`, drawn as a PNG. */
export default function AppleIcon() {
  return new ImageResponse(
    <div
      style={{
        width: "100%",
        height: "100%",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        backgroundColor: "#1a1917",
      }}
    >
      <Mark width={112} height={112} style={{ color: "#fbfaf8" }} />
    </div>,
    size,
  );
}
