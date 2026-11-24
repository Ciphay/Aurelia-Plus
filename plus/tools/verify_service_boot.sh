#!/usr/bin/env bash
here=$(cd "$(dirname "$0")" && pwd)
A=${1:-$here/../../aurelia/target/release/aurelia}
ok=0; bad=0
check() { if [ "$2" = ok ]; then echo "OK   $1"; ok=$((ok+1)); else echo "FAIL $1 ($2)"; bad=$((bad+1)); fi; }

boot=$(date -d "$(uptime -s)" +%s)
echo "boot at $(uptime -s) ($(( $(date +%s) - boot )) s ago)"
[ "$(systemctl --user is-enabled aurelia-daemon.socket 2>/dev/null)" = enabled ] && check "socket enabled" ok || check "socket enabled" "run: aurelia service install"
[ "$(systemctl --user is-active aurelia-daemon.socket)" = active ] && check "socket active without anyone having called it" ok || check "socket active" "inactive"
[ "$(systemctl --user is-active aurelia-daemon.service)" = active ] && check "daemon started by itself at login" ok || check "daemon started by itself at login" "inactive"

start=$(date -d "$(systemctl --user show aurelia-daemon.service -p ActiveEnterTimestamp --value)" +%s 2>/dev/null || echo 0)
echo "daemon active $((start - boot)) s after boot"

echo; echo "--- critical chain of the login (default.target) and cost of the units"
systemd-analyze --user critical-chain default.target 2>&1 | head -8
systemd-analyze --user blame 2>&1 | grep -i aurelia

echo; echo "--- warm-up (waits up to 3 min: the network may take a while to come up at login)"
log=~/.config/Aurelia/logs/bridge.log
warmed() { awk -v b="$boot" '/daemon: account warmed/ && $1 >= b {found=1} END {exit !found}' "$log" 2>/dev/null; }
for _ in $(seq 1 90); do warmed && break; sleep 2; done
warmed && check "account warmed up after boot ('account warmed' line in bridge.log)" ok || check "account warmed up after boot" "no 'account warmed' line (no network or not logged in?)"

echo; "$A" bridge status
echo; echo "$ok ok, $bad failure(s)"; [ "$bad" = 0 ]
