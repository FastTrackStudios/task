/**
 * Header account switcher — avatar initials + name, opening the dev
 * account roster. Real sign-in against the home org (lib/auth.tsx);
 * identity is display-only for now but rides context for later use.
 */
import { Check, LogOut } from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { AccountAvatar, DEV_ACCOUNTS, useAccount } from "@/lib/auth";
import { cn } from "@/lib/utils";

export function AccountSwitcher() {
  const { account, busy, error, switchAccount, signOut } = useAccount();

  const name = account?.name ?? (busy ? "Signing in…" : "Signed out");
  const email = account?.email ?? "";

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className={cn(
          "flex h-7 items-center gap-2 rounded-md px-1.5 text-sm",
          "hover:bg-accent transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
          busy && "opacity-70",
        )}
        title={error ? `auth: ${error}` : email || name}
      >
        <AccountAvatar name={account?.name ?? "?"} email={email} size={22} />
        <span className="hidden max-w-32 truncate font-medium sm:block">
          {name}
        </span>
        {error && (
          <span
            className="bg-destructive size-1.5 rounded-full"
            title={error}
          />
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64">
        <DropdownMenuLabel className="text-muted-foreground text-xs">
          Account
        </DropdownMenuLabel>
        {DEV_ACCOUNTS.map((dev) => (
          <DropdownMenuItem
            key={dev.email}
            disabled={busy}
            onSelect={() => switchAccount(dev.email)}
          >
            <AccountAvatar name={dev.name} email={dev.email} size={22} />
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="truncate">{dev.name}</span>
              <span className="text-muted-foreground truncate text-[10px]">
                {dev.email}
              </span>
            </span>
            {account?.email === dev.email && <Check className="size-3.5" />}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem
          variant="destructive"
          disabled={busy || !account}
          onSelect={() => signOut()}
        >
          <LogOut className="size-3.5" />
          Sign out
        </DropdownMenuItem>
        {error && (
          <p className="text-destructive px-2 pb-1.5 pt-1 text-[11px]">
            {error}
          </p>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
