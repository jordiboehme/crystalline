/**
 * What the avatar menu holds (spec 4.3): who you are, the doors only an
 * administrator has, Maintenance, the Display group, the shortcut map, the
 * Handbook and Log out. The anonymous viewer gets the same menu without the
 * account rows and with Log in in their place.
 *
 * Lazy: only the trigger is in the entry (`AvatarMenu.tsx`).
 *
 * The three Display choices are segmented radio groups inside one row each.
 * Every segment is a menu radio item whose select does not close the menu, so
 * a reader can try Dark and then Large in one visit. Up and Down walk every
 * row as in any menu; Left and Right move within a group.
 */

import {
  BookOpen,
  Check,
  ExternalLink,
  Keyboard,
  LogIn,
  LogOut,
  Moon,
  MoveHorizontal,
  Settings,
  Stethoscope,
  Sun,
  ALargeSmall,
  User,
  UserRoundX,
  Users,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import type { KeyboardEvent, ReactElement, ReactNode } from "react";
import { Link, useMatch } from "react-router";

import { useAuth } from "../auth/AuthContext";
import { HANDBOOK_URL } from "../handbook";
import { useLargeText } from "../layoutText";
import { useFullWidth } from "../layoutWidth";
import { useMarkSetting } from "../markSetting/context";
import {
  githubSettingsRoute,
  maintenanceRoute,
  profileRoute,
  usersRoute,
} from "../paths";
import { PREFETCH } from "../prefetch";
import { useTheme } from "../theme/context";
import { useWide } from "../useWide";
import type { ThemePreference } from "../theme/context";
import { ITEM_CLASSES, MENU_SEPARATOR } from "./menu";

const ICON = "shrink-0 text-muted";
const SEGMENT =
  "inline-grid h-8 min-w-[30px] cursor-pointer place-items-center rounded-[5px] px-2 text-[13px] text-muted outline-none select-none data-[highlighted]:text-ink data-[highlighted]:ring-2 data-[highlighted]:ring-accent-600 data-[state=checked]:bg-seg-on data-[state=checked]:text-ink data-[state=checked]:shadow-[0_1px_2px_rgb(15_23_42/0.1)] md:h-6 md:text-xs dark:data-[highlighted]:ring-accent-400";

/** Left and Right move between the segments of the group the key came from. */
function walkGroup(event: KeyboardEvent<HTMLDivElement>): void {
  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
    return;
  }
  const items = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>('[role="menuitemradio"]'),
  );
  const at = items.indexOf(document.activeElement as HTMLElement);
  if (at < 0) {
    return;
  }
  const step = event.key === "ArrowRight" ? 1 : -1;
  items[(at + step + items.length) % items.length]?.focus();
  event.preventDefault();
}

/** One Display row: an icon and a name, and the choices as a segmented group. */
function Choice<T extends string>({
  name,
  icon,
  value,
  options,
  onChoose,
}: {
  name: string;
  icon: ReactNode;
  value: T;
  options: { value: T; label: string }[];
  onChoose: (value: T) => void;
}): ReactElement {
  return (
    <div className="flex min-h-11 items-center gap-2.5 px-2.5 md:min-h-[34px]">
      {icon}
      <span aria-hidden="true" className="text-ink-2">
        {name}
      </span>
      <DropdownMenu.RadioGroup
        aria-label={name}
        value={value}
        onValueChange={(next) => {
          onChoose(next as T);
        }}
        onKeyDown={walkGroup}
        className="ml-auto inline-flex gap-0.5 rounded-[9px] bg-seg p-0.5 md:rounded-[7px]"
      >
        {options.map((option) => (
          <DropdownMenu.RadioItem
            key={option.value}
            value={option.value}
            className={SEGMENT}
            onSelect={(event) => {
              event.preventDefault();
            }}
          >
            {option.label}
          </DropdownMenu.RadioItem>
        ))}
      </DropdownMenu.RadioGroup>
    </div>
  );
}

const THEMES: { value: ThemePreference; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

/** The account rows, or the anonymous viewer's one line. */
function Who(): ReactElement {
  const { user, capabilities } = useAuth();
  if (!user) {
    return (
      <div className="flex items-center gap-3 px-2.5 pt-2.5 pb-3">
        <span className="avatar avatar-anon">
          <UserRoundX aria-hidden="true" size={16} strokeWidth={1.75} />
        </span>
        <DropdownMenu.Label className="text-muted">
          No account on this session
        </DropdownMenu.Label>
      </div>
    );
  }
  const role = capabilities.role ?? "viewer";
  const roleWords = `${role.charAt(0).toUpperCase()}${role.slice(1)} on this instance`;
  return (
    <div className="flex items-center gap-3 px-2.5 pt-2.5 pb-3">
      <span aria-hidden="true" className="avatar avatar-lg">
        {(Array.from(user.display.trim())[0] ?? "?").toUpperCase()}
      </span>
      <DropdownMenu.Label className="min-w-0">
        <b className="block truncate text-sm font-semibold text-ink">
          {user.display}
        </b>
        <span className="block truncate text-[12.5px] text-muted">
          {user.name === user.display ? "" : `${user.name} - `}
          {roleWords}
        </span>
      </DropdownMenu.Label>
    </div>
  );
}

export default function AvatarMenuBody({
  onShortcuts,
}: {
  onShortcuts: () => void;
}): ReactElement {
  const { user, capabilities, logout } = useAuth();
  const { preference, resolved, choose } = useTheme();
  const { largeText, toggleLargeText } = useLargeText();
  const { fullWidth, toggleFullWidth } = useFullWidth();
  const { showOnEditLine, setShowOnEditLine } = useMarkSetting();
  const wide = useWide();
  const onMaintenance = useMatch(maintenanceRoute()) !== null;
  const glyph = { "aria-hidden": true, size: 16, strokeWidth: 1.75 } as const;

  return (
    <>
      <div aria-hidden="true" className="avatar-grab" />
      <Who />
      {user && (
        <>
          {/*
            Ungated, unlike the two rows under it: this one is about the
            account named above rather than about the instance, so every
            signed-in session has one to open. A viewer's says why there is no
            identity to connect there, which is an answer rather than a door
            that will not open.
          */}
          <DropdownMenu.Item className={ITEM_CLASSES} asChild>
            <Link to={profileRoute()} {...PREFETCH.profile}>
              <User {...glyph} className={ICON} />
              Profile
            </Link>
          </DropdownMenu.Item>
          {capabilities.canAdminister && (
            <>
              <DropdownMenu.Item className={ITEM_CLASSES} asChild>
                <Link to={usersRoute()} {...PREFETCH.users}>
                  <Users {...glyph} className={ICON} />
                  Users
                </Link>
              </DropdownMenu.Item>
              {/*
                Named for what the screen is rather than for the service it
                talks to: it is where this instance's settings are kept, and
                the one page of them today is the repository connection team
                domains are tracked with.
              */}
              <DropdownMenu.Item className={ITEM_CLASSES} asChild>
                <Link to={githubSettingsRoute()} {...PREFETCH.github}>
                  <Settings {...glyph} className={ICON} />
                  Settings
                </Link>
              </DropdownMenu.Item>
            </>
          )}
        </>
      )}
      {!user && (
        <DropdownMenu.Item className={ITEM_CLASSES} asChild>
          <Link to="/login">
            <LogIn {...glyph} className={ICON} />
            Log in
          </Link>
        </DropdownMenu.Item>
      )}
      {/*
        A plain link with the active state worked out here: the item's slot
        joins class names as text, which a NavLink's class function does not
        survive.
      */}
      <DropdownMenu.Item
        className={
          onMaintenance
            ? `${ITEM_CLASSES} bg-accent-tint font-medium text-ink`
            : ITEM_CLASSES
        }
        asChild
      >
        <Link
          to={maintenanceRoute()}
          {...PREFETCH.maintenance}
          aria-current={onMaintenance ? "page" : undefined}
        >
          <Stethoscope {...glyph} className={ICON} />
          Maintenance
        </Link>
      </DropdownMenu.Item>

      <DropdownMenu.Separator className={MENU_SEPARATOR} />
      <DropdownMenu.Group aria-label="Display">
        <div className="hidden px-2.5 pt-1 pb-1 text-[11.5px] font-semibold tracking-wider text-faint uppercase md:block">
          Display
        </div>
        <Choice
          name="Theme"
          icon={
            resolved === "dark" ? (
              <Moon {...glyph} className={ICON} />
            ) : (
              <Sun {...glyph} className={ICON} />
            )
          }
          value={preference}
          options={THEMES}
          onChoose={choose}
        />
        <Choice
          name="Text size"
          icon={<ALargeSmall {...glyph} className={ICON} />}
          value={largeText ? "large" : "regular"}
          options={[
            { value: "regular", label: "Regular" },
            { value: "large", label: "Large" },
          ]}
          onChoose={(next) => {
            if ((next === "large") !== largeText) {
              toggleLargeText();
            }
          }}
        />
        {/* Below the medium breakpoint the content is always full width, so the choice is not offered there. */}
        {wide && (
          <Choice
            name="Width"
            icon={<MoveHorizontal {...glyph} className={ICON} />}
            value={fullWidth ? "full" : "reading"}
            options={[
              { value: "reading", label: "Reading" },
              { value: "full", label: "Full" },
            ]}
            onChoose={(next) => {
              if ((next === "full") !== fullWidth) {
                toggleFullWidth();
              }
            }}
          />
        )}
        <DropdownMenu.CheckboxItem
          className={ITEM_CLASSES}
          checked={showOnEditLine}
          onCheckedChange={setShowOnEditLine}
          onSelect={(event) => {
            event.preventDefault();
          }}
        >
          <span className="grid size-4 shrink-0 place-items-center">
            <DropdownMenu.ItemIndicator>
              <Check {...glyph} className="text-ink" />
            </DropdownMenu.ItemIndicator>
          </span>
          Show Markdown marks on the line I edit
        </DropdownMenu.CheckboxItem>
      </DropdownMenu.Group>

      <DropdownMenu.Separator className={MENU_SEPARATOR} />
      {/* Kept on a phone too: a tablet in split view and a phone with a hardware keyboard have the keys. */}
      <DropdownMenu.Item className={ITEM_CLASSES} onSelect={onShortcuts}>
        <Keyboard {...glyph} className={ICON} />
        Keyboard shortcuts
        <kbd
          aria-hidden="true"
          className="text-caption ml-auto rounded border border-hair-strong px-1.5 text-faint"
        >
          ?
        </kbd>
      </DropdownMenu.Item>
      <DropdownMenu.Item className={ITEM_CLASSES} asChild>
        <a href={HANDBOOK_URL} target="_blank" rel="noreferrer">
          <BookOpen {...glyph} className={ICON} />
          Handbook <span className="sr-only">(opens in a new tab)</span>
          <ExternalLink
            aria-hidden="true"
            size={14}
            strokeWidth={1.75}
            className="ml-auto text-faint"
          />
        </a>
      </DropdownMenu.Item>
      {user && (
        <>
          <DropdownMenu.Separator className={MENU_SEPARATOR} />
          <DropdownMenu.Item
            className={ITEM_CLASSES}
            onSelect={() => {
              void logout();
            }}
          >
            <LogOut {...glyph} className={ICON} />
            Log out
          </DropdownMenu.Item>
        </>
      )}
    </>
  );
}
