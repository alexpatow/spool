// The controls of a form, each with the words that say what it is for.

import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { Checkbox } from "spool-ui/components/checkbox";
import * as NativeSelect from "spool-ui/components/native-select";
import { colors, spacing, typography } from "spool-ui/tokens.stylex";

/** A control under the words that say what it is for. */
export function Labelled({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label title={hint} {...stylex.props(styles.labelled)}>
      <span {...stylex.props(styles.label)}>{label}</span>
      {children}
    </label>
  );
}

/** A box to tick, with the words that say what ticking it means. */
export function Flag({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label {...stylex.props(styles.flag)}>
      <Checkbox aria-label={label} checked={checked} onCheckedChange={(next) => onChange(next === true)} />
      <span aria-hidden>{label}</span>
    </label>
  );
}

export interface Choice {
  value: string;
  label?: string;
}

/** One of a few values, chosen from the browser's own list. */
export function Select({
  name,
  value,
  defaultValue,
  onChange,
  choices,
  any,
}: {
  name?: string;
  value?: string;
  defaultValue?: string;
  onChange?: (value: string) => void;
  choices: Choice[];
  /** What the choice of no value is called. Without it there is no such choice. */
  any?: string;
}) {
  return (
    <NativeSelect.Root
      name={name}
      value={value}
      defaultValue={defaultValue}
      onChange={onChange ? (event) => onChange(event.target.value) : undefined}
    >
      {any !== undefined ? <NativeSelect.Option value="">{any}</NativeSelect.Option> : null}
      {choices.map((choice) => (
        <NativeSelect.Option key={choice.value} value={choice.value}>
          {choice.label ?? choice.value.replaceAll("_", " ")}
        </NativeSelect.Option>
      ))}
    </NativeSelect.Root>
  );
}

const styles = stylex.create({
  labelled: {
    gap: spacing["1.5"],
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  label: {
    color: colors.mutedForeground,
    fontSize: typography.fontSizeXs,
    fontWeight: typography.fontWeightMedium,
    lineHeight: typography.lineHeightXs,
  },
  flag: {
    gap: spacing["2"],
    alignItems: "center",
    cursor: "pointer",
    display: "inline-flex",
    minHeight: "2rem",
  },
});
