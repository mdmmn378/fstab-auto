#!/usr/bin/env bash
# End-to-end test. Runs inside the `client` container of docker/compose.yaml:
# drives the real TUI through tmux, as an unprivileged user, against a real
# Samba server, and checks what actually happened to the system.
set -euo pipefail

SESSION=e2e
NAS=nas01
PASSWORD=${NAS_PASSWORD:?}
FSTAB=/etc/fstab
CREDS=/etc/fstab-auto/credentials

# Everything TUI-related runs as the unprivileged user, like on a real machine.
t() { sudo -u alice tmux -L e2e "$@"; }
screen() { t capture-pane -p -t "$SESSION"; }
key() { t send-keys -t "$SESSION" "$@"; }
type_text() { t send-keys -t "$SESSION" -l -- "$1"; }

section() { printf '\n== %s\n' "$*"; }
ok() { printf '   ok  %s\n' "$*"; }
fail() {
    printf '\nFAIL: %s\n----- screen -----\n' "$*"
    screen || true
    exit 1
}
check() {
    local what=$1
    shift
    "$@" || fail "$what"
    ok "$what"
}

wait_for() {
    local text=$1 tries=${2:-150}
    for _ in $(seq "$tries"); do
        if screen | grep -qF -- "$text"; then return 0; fi
        sleep 0.1
    done
    fail "timed out waiting for: $text"
}
on_screen() { screen | grep -qF -- "$1"; }
not_on_screen() { ! on_screen "$1"; }

start() {
    t kill-server 2>/dev/null || true
    t new-session -d -s "$SESSION" -x 200 -y 45 "fstab-auto $*; echo \"exited: \$?\"; sleep 600"
    wait_for "Step 1 of"
}
quit() {
    key C-c
    wait_for "exited: 0"
}

# Answers every step up to (and not including) the persistence choice.
#   $1 share  $2 username ("" = guest)  $3 password  $4 mount dir
answer_until_mode() {
    local share=$1 user=$2 password=$3 dir=$4
    key Enter
    wait_for "Which NAS or file server"
    type_text "$NAS"
    key Enter
    wait_for "Which share on $NAS"
    type_text "$share"
    key Enter
    wait_for "Which account"
    if [[ -z $user ]]; then
        key C-u
        key Enter
    else
        key Enter
        wait_for "Password for $user on $NAS"
        type_text "$password"
        not_on_screen "$password" || fail "password is visible while typing"
        key Enter
    fi
    wait_for "Where on this machine"
    key C-u
    type_text "$dir"
    key Enter
    if [[ ! -d $dir ]]; then
        wait_for "Create directory?"
        key y
    fi
    wait_for "Owner UID"
    key Enter
    wait_for "How long should the share stay mounted"
}

section "Waiting for the NAS"
for _ in $(seq 100); do
    if (exec 3<>"/dev/tcp/$NAS/445") 2>/dev/null; then break; fi
    sleep 0.2
done
check "NAS is reachable" bash -c "exec 3<>/dev/tcp/$NAS/445"
cp "$FSTAB" /tmp/fstab.orig

section "Temporary mount (as unprivileged user, through sudo)"
start
check "runs with a DRY RUN badge absent" not_on_screen "DRY RUN"
key Enter
wait_for "Which NAS or file server"
key Escape
wait_for "How does the NAS share its files"
answer_until_mode projects alice "$PASSWORD" /mnt/nas
check "owner defaults to the invoking user, not root" on_screen "uid=1001,gid=1001"
key Enter
wait_for "Press Enter to mount"
check "review shows the equivalent command" on_screen \
    "mount -t cifs -o username=alice,uid=1001,gid=1001 //$NAS/projects /mnt/nas"
check "review announces directory creation" on_screen "will be created"
check "review hides the password" not_on_screen "$PASSWORD"
key Enter
wait_for "//$NAS/projects is mounted on /mnt/nas" 300
check "directory was created" test -d /mnt/nas
check "share is mounted" mountpoint -q /mnt/nas
check "files are readable" grep -q "hello from the nas" /mnt/nas/hello.txt
check "files belong to uid 1001" test "$(stat -c %u /mnt/nas/hello.txt)" = 1001
check "user can write" sudo -u alice touch /mnt/nas/written-by-client
check "fstab is untouched" cmp -s "$FSTAB" /tmp/fstab.orig
check "no credentials were stored" test ! -e "$CREDS"
check "password is not in any process list" bash -c "! ps axww -o args | grep -F -- '$PASSWORD' | grep -v grep"

key Tab
wait_for "Network shares"
check "Mounts tab lists it as mounted" on_screen "● mounted"
check "Mounts tab lists it as temporary" on_screen "temporary"
key u
wait_for "Unmount?"
key y
wait_for "Unmounted /mnt/nas"
check "share is unmounted" bash -c "! mountpoint -q /mnt/nas"
quit

section "Permanent mount"
start
answer_until_mode projects alice "$PASSWORD" /mnt/nas
key Down
key Enter
wait_for "Press Enter to mount"
LINE="//$NAS/projects /mnt/nas cifs credentials=$CREDS/mnt-nas.cred,uid=1001,gid=1001,_netdev,nofail 0 0"
check "review shows the fstab entry" on_screen "$LINE"
key Enter
wait_for "//$NAS/projects is mounted on /mnt/nas" 300
check "result explains persistence" on_screen "mounted again automatically on every boot"
check "share is mounted" mountpoint -q /mnt/nas
check "fstab has the entry" grep -qxF -- "$LINE" "$FSTAB"
check "entry is marked as ours" grep -qxF "# Added by fstab-auto" "$FSTAB"
check "original fstab lines are preserved" bash -c "head -n \$(wc -l < /tmp/fstab.orig) $FSTAB | cmp -s - /tmp/fstab.orig"
check "fstab was backed up" cmp -s "$FSTAB.fstab-auto.bak" /tmp/fstab.orig
check "credentials file is root-only" test "$(stat -c '%a %U' "$CREDS/mnt-nas.cred")" = "600 root"
check "credentials dir is root-only" test "$(stat -c '%a %U' "$CREDS")" = "700 root"
check "credentials hold the password" grep -qxF -- "password=$PASSWORD" "$CREDS/mnt-nas.cred"
check "password is not in fstab" bash -c "! grep -qF -- '$PASSWORD' $FSTAB"

umount /mnt/nas
mount -a
check "survives a simulated reboot (umount + mount -a)" mountpoint -q /mnt/nas
check "file written earlier is still there" test -e /mnt/nas/written-by-client

key Tab
wait_for "Network shares"
check "Mounts tab lists it as permanent" on_screen "permanent"
key d
wait_for "Remove permanent mount?"
key Enter
check "removal defaults to No" grep -qxF -- "$LINE" "$FSTAB"
key d
wait_for "Remove permanent mount?"
key y
wait_for "Removed /mnt/nas from $FSTAB"
check "share is unmounted" bash -c "! mountpoint -q /mnt/nas"
check "fstab is back to the original" cmp -s "$FSTAB" /tmp/fstab.orig
check "credentials file is deleted" test ! -e "$CREDS/mnt-nas.cred"
quit

section "Wrong password rolls everything back"
start
answer_until_mode projects alice "not-the-password" /mnt/nas2
key Down
key Enter
wait_for "Press Enter to mount"
key Enter
wait_for "The share was not mounted" 300
check "explains the failure" on_screen "The NAS rejected the login"
check "reports the rollback" on_screen "Rolled back"
check "nothing is mounted" bash -c "! mountpoint -q /mnt/nas2"
check "fstab is unchanged" cmp -s "$FSTAB" /tmp/fstab.orig
check "no credentials left behind" test ! -e "$CREDS/mnt-nas2.cred"
key Enter
wait_for "Press Enter to mount"
ok "Enter returns to the review step"
quit

section "Guest access, unknown share"
start
answer_until_mode public "" "" /mnt/public
key Enter
wait_for "Press Enter to mount"
check "guest command has no username" on_screen "mount -t cifs -o guest,uid=1001,gid=1001 //$NAS/public /mnt/public"
key Enter
wait_for "//$NAS/public is mounted on /mnt/public" 300
check "guest share is readable" grep -q "public file" /mnt/public/readme.txt
umount /mnt/public
key Enter
wait_for "Which share on $NAS"
ok "Enter starts another mount with the server remembered"
type_text "no_such_share"
key Enter
wait_for "Which account"
key Enter
wait_for "Where on this machine"
check "mount directory is suggested from the share name" on_screen "/mnt/no_such_share"
key Enter
wait_for "Create directory?"
key y
wait_for "Owner UID"
key Enter
wait_for "How long should"
key Enter
wait_for "Press Enter to mount"
key Enter
wait_for "The share was not mounted" 300
check "explains a missing share" on_screen "no share with that name"
quit

section "Dry run needs no privileges"
start --dry-run
check "shows the DRY RUN badge" on_screen "DRY RUN"
quit
check "fstab is unchanged at the end" cmp -s "$FSTAB" /tmp/fstab.orig

printf '\nAll end-to-end checks passed.\n'
