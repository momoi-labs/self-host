#!/usr/bin/env bash
#
# Validates removing an Application with its data and `prune` (#200) on a real
# Host, Linux or macOS. It creates a throwaway Compose Application with a named
# volume, then:
#
#   1. removes it keeping its data, checks the volume is still there and that
#      `prune` lists and then removes what it left;
#   2. creates it again and removes it with delete_data, checks nothing is left.
#
# It touches only the Applications it creates, named cleanup-check-*, and
# never runs `prune` while something else would be removed: if the plan holds
# anything that is not its own, it stops and shows it instead.
#
# Run it as the user the Platform runs as: the Operator on macOS, root on a
# Linux Host installed from the package (sudo -E bash validate-cleanup.sh).
#
# Environment:
#   SELF_HOST_API   API base URL   (default http://127.0.0.1:3721)
#   SELF_HOST_KEY   API key        (default: `self-host init --show-key`)

set -euo pipefail

API="${SELF_HOST_API:-http://127.0.0.1:3721}"
KEY="${SELF_HOST_KEY:-}"
IMAGE="busybox:1.36"
FAILED=0
CREATED=()

if [ -z "$KEY" ]; then
	KEY="$(self-host init --show-key 2>/dev/null | tail -1 | tr -d '[:space:]')"
fi
[ -n "$KEY" ] || { echo "no API key; set SELF_HOST_KEY" >&2; exit 2; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }
command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }

api() {
	curl -sS -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' "$@"
}
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }
pass() { echo "  PASS  $1"; }
fail() { echo "  FAIL  $1"; FAILED=1; }

# A task is done when its event says so.
wait_task() {
	local task="$1" status i
	for i in $(seq 1 180); do
		status="$(api "$API/events" | python3 -c '
import sys, json
task = sys.argv[1]
event = next((e for e in json.load(sys.stdin) if e.get("id") == task), {})
print(event.get("status", ""))' "$task")"
		case "$status" in completed | failed) echo "$status"; return ;; esac
		sleep 1
	done
	echo timeout
}

# An Application is up once it leaves pending.
wait_running() {
	local id="$1" status i
	for i in $(seq 1 180); do
		status="$(api "$API/apps/id/$id" | json 'd.get("status","")')"
		case "$status" in running | failed) echo "$status"; return ;; esac
		sleep 1
	done
	echo timeout
}

create() {
	local name="cleanup-check-$(date +%s)-$RANDOM"
	local body
	body="$(python3 -c '
import json, sys
compose = "services:\n  store:\n    image: %s\n    command: [\"sleep\", \"1d\"]\n    volumes:\n      - data:/data\nvolumes:\n  data: {}\n" % sys.argv[2]
print(json.dumps({"name": sys.argv[1], "compose": compose, "publication": {"kind": "unpublished"}}))' "$name" "$IMAGE")"
	api "$API/apps" -d "$body" | json 'd["id"]'
}

remove() {
	local id="$1" query="${2:-}"
	api -X DELETE "$API/apps/id/$id$query" | json 'd["task_id"]'
}

# Lines of the prune plan, as "kind name".
plan() {
	api "$API/prune" | python3 -c '
import sys, json
for item in json.load(sys.stdin)["items"]:
    print(item["kind"], item["name"])'
}

volume_exists() { docker volume inspect "sf-app-$1_data" >/dev/null 2>&1; }

cleanup() {
	# Whatever this run created and did not get to remove goes, data and all.
	local id
	for id in "${CREATED[@]:-}"; do
		[ -n "$id" ] || continue
		api -X DELETE "$API/apps/id/$id?delete_data=true" >/dev/null 2>&1 || true
	done
}
trap cleanup EXIT

echo "Platform at $API"
if ! api -f "$API/prune" >/dev/null 2>&1; then
	echo "This Platform has no /prune; it needs a build with #200 (PR #202)." >&2
	exit 2
fi

before="$(plan)"
if [ -n "$before" ]; then
	echo
	echo "prune already has something to remove on this Host:"
	echo "$before" | sed 's/^/    /'
	echo "This run checks prune by running it, which would remove those too."
	echo "Run 'self-host prune' first if they are garbage, then run this again."
	exit 3
fi

echo
echo "1. Remove keeping the data, then prune"
id="$(create)"
CREATED+=("$id")
status="$(wait_running "$id")"
[ "$status" = running ] && pass "created $id" || { fail "create $id ended $status"; exit 1; }
docker exec "sf-app-$id-store" sh -c 'echo kept > /data/marker' && pass "wrote to its volume" || fail "could not write to its volume"

[ "$(wait_task "$(remove "$id")")" = completed ] && pass "removed, keeping data" || fail "remove did not complete"
volume_exists "$id" && pass "volume sf-app-${id}_data is still there" || fail "volume went without delete_data"

listed="$(plan)"
echo "$listed" | grep -q "^volume sf-app-${id}_data$" && pass "prune lists the volume" || fail "prune does not list the volume"
echo "$listed" | grep -q "^history $id$" && pass "prune lists the deployment history" || fail "prune does not list the history"
# The image this run pulled has no id in its name; anything else is not ours.
others="$(echo "$listed" | grep -v "$id" | grep -vx "image $IMAGE" || true)"
if [ -n "$others" ]; then
	fail "prune lists something that is not this run's:"
	echo "$others" | sed 's/^/    /'
	exit 1
fi

result="$(api -X POST "$API/prune" | json 'd["task_id"]')"
[ "$(wait_task "$result")" = completed ] && pass "prune ran" || fail "prune did not complete"
volume_exists "$id" && fail "volume is still there after prune" || pass "prune removed the volume"
[ -z "$(plan)" ] && pass "nothing left to prune" || fail "prune still lists: $(plan | tr '\n' ' ')"

echo
echo "2. Remove with delete_data"
id="$(create)"
CREATED+=("$id")
status="$(wait_running "$id")"
[ "$status" = running ] && pass "created $id" || { fail "create $id ended $status"; exit 1; }
[ "$(wait_task "$(remove "$id" '?delete_data=true')")" = completed ] && pass "removed with its data" || fail "remove did not complete"
volume_exists "$id" && fail "volume is still there" || pass "volume went with it"
[ -z "$(plan)" ] && pass "nothing left for prune" || fail "prune still lists: $(plan | tr '\n' ' ')"
if docker image inspect "$IMAGE" >/dev/null 2>&1; then
	users="$(docker ps -a --filter "ancestor=$IMAGE" --format '{{.Names}}' | tr '\n' ' ')"
	if [ -n "$users" ]; then
		pass "$IMAGE stays: other containers use it ($users)"
	else
		fail "$IMAGE is still on the Host and nothing uses it"
	fi
else
	pass "$IMAGE went with it"
fi
CREATED=()

echo
if [ "$FAILED" -eq 0 ]; then echo "All checks passed."; else echo "Some checks failed."; exit 1; fi
