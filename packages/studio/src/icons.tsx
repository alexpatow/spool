// The icons the screens use that the components do not bring. Like theirs,
// each is as large as the text beside it and says nothing to a screen reader.

import {
  ActivityIcon as Activity,
  ArrowDownIcon as ArrowDown,
  ArrowUpDownIcon as ArrowUpDown,
  ArrowUpIcon as ArrowUp,
  ArrowRightIcon as ArrowRight,
  FilePenLineIcon as FilePenLine,
  FingerprintIcon as Fingerprint,
  GitCompareArrowsIcon as GitCompareArrows,
  GitMergeIcon as GitMerge,
  KeySquareIcon as KeySquare,
  KeyRoundIcon as KeyRound,
  LogOutIcon as LogOut,
  RefreshCwIcon as RefreshCw,
  Rows3Icon as Rows3,
  ScrollTextIcon as ScrollText,
  SplitIcon as Split,
  Undo2Icon as Undo2,
  UserPlusIcon as UserPlus,
  UsersRoundIcon as UsersRound,
} from "lucide-react";
import type { SVGProps } from "react";

export {
  CheckIcon,
  CircleAlertIcon,
  CircleCheckIcon,
  DarkModeIcon,
  FileTextIcon,
  InboxIcon,
  InfoIcon,
  LayoutDashboardIcon,
  LightModeIcon,
  PlusIcon,
  SearchIcon,
} from "spool-ui/components/icons";

type IconProps = Omit<SVGProps<SVGSVGElement>, "ref">;

const DEFAULTS = { "aria-hidden": true, height: "1em", width: "1em" } as const;

export const ArrowDownIcon = (props: IconProps) => <ArrowDown {...DEFAULTS} {...props} />;
export const ArrowRightIcon = (props: IconProps) => <ArrowRight {...DEFAULTS} {...props} />;
export const ArrowUpIcon = (props: IconProps) => <ArrowUp {...DEFAULTS} {...props} />;
export const ArrowUpDownIcon = (props: IconProps) => <ArrowUpDown {...DEFAULTS} {...props} />;
export const DraftIcon = (props: IconProps) => <FilePenLine {...DEFAULTS} {...props} />;
export const ExplainIcon = (props: IconProps) => <GitCompareArrows {...DEFAULTS} {...props} />;
export const IdentityIcon = (props: IconProps) => <Fingerprint {...DEFAULTS} {...props} />;
export const LogIcon = (props: IconProps) => <ScrollText {...DEFAULTS} {...props} />;
export const MergeIcon = (props: IconProps) => <GitMerge {...DEFAULTS} {...props} />;
export const RecordIcon = (props: IconProps) => <Rows3 {...DEFAULTS} {...props} />;
export const RefreshIcon = (props: IconProps) => <RefreshCw {...DEFAULTS} {...props} />;
export const RunIcon = (props: IconProps) => <Activity {...DEFAULTS} {...props} />;
export const SplitIcon = (props: IconProps) => <Split {...DEFAULTS} {...props} />;
export const PasswordIcon = (props: IconProps) => <KeyRound {...DEFAULTS} {...props} />;
export const PeopleIcon = (props: IconProps) => <UsersRound {...DEFAULTS} {...props} />;
export const TokenIcon = (props: IconProps) => <KeySquare {...DEFAULTS} {...props} />;
export const SignOutIcon = (props: IconProps) => <LogOut {...DEFAULTS} {...props} />;
export const UserPlusIcon = (props: IconProps) => <UserPlus {...DEFAULTS} {...props} />;
export const UndoIcon = (props: IconProps) => <Undo2 {...DEFAULTS} {...props} />;
