#!/usr/bin/env bash
# Этап 9, S9-42: воспроизводимый локальный набор проверок.
#
# Результаты пишутся в ignored work/stage9/<run-id>/ (results.tsv, environment.txt,
# frozen.txt, logs/). В Git лежат только этот скрипт и docs/testing/STAGE_9_REPRO.md.
#
# Статусы case: passed / failed / not_run / blocked.
#  - passed ставится только после реального запуска с кодом выхода 0;
#  - отсутствующий инструмент даёт not_run, превышение бюджета времени даёт not_run;
#  - hardware/Windows/review кейсы записываются статично (blocked / not_run) и
#    никогда не заполняются локальным успехом.
#
# Переменные окружения:
#   STAGE9_RUN_ID                run-id (по умолчанию дата-время)
#   STAGE9_BUDGET_SECONDS        общий бюджет на набор (по умолчанию 840)
#   STAGE9_CASE_TIMEOUT          таймаут одного case, секунды (по умолчанию 600)
#   STAGE9_LAYOUT_ERROR_BASELINE известное число ERROR у tools/check_layout.py (по умолчанию 45)
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT" || exit 2

RUN_ID="${STAGE9_RUN_ID:-$(date +%Y%m%d-%H%M%S)}"
BUDGET="${STAGE9_BUDGET_SECONDS:-840}"
CASE_TIMEOUT="${STAGE9_CASE_TIMEOUT:-600}"
LAYOUT_BASELINE="${STAGE9_LAYOUT_ERROR_BASELINE:-45}"

OUT="$ROOT/work/stage9/$RUN_ID"
LOGS="$OUT/logs"
# Прогон не перезаписывает старый run-каталог (STAGE_9_REPRO.md §5).
if [ -e "$OUT" ]; then
  echo "ERROR: $OUT уже существует; задайте новый STAGE9_RUN_ID" >&2
  exit 2
fi
mkdir -p "$LOGS" || exit 2
RESULTS="$OUT/results.tsv"
START_EPOCH=$(date +%s)

# --- Игнорируется ли work/ Git'ом -------------------------------------------
if git check-ignore -q "work/stage9/$RUN_ID/results.tsv"; then
  IGNORED=yes
else
  IGNORED=NO
  echo "WARNING: work/stage9/$RUN_ID не игнорируется Git; не добавлять в коммит." >&2
fi

# --- Заморозка commit/config/artifact hashes и окружение --------------------
hash_of() {
  if [ -f "$1" ]; then shasum -a 256 "$1" | awk '{print $1}'; else echo "missing"; fi
}
{
  echo "run_id: $RUN_ID"
  echo "started_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "work_dir_ignored_by_git: $IGNORED"
  echo "git_head: $(git rev-parse HEAD 2>&1)"
  echo "git_branch: $(git rev-parse --abbrev-ref HEAD 2>&1)"
  echo "git_status_porcelain:"
  git status --porcelain 2>&1 | sed 's/^/  /'
  echo "sw_vers:"
  (sw_vers 2>&1 || echo "sw_vers: not available") | sed 's/^/  /'
  echo "uname: $(uname -a 2>&1)"
  echo "rustc: $(rustc --version 2>&1)"
  echo "cargo: $(cargo --version 2>&1)"
  echo "python3: $(python3 --version 2>&1)"
  echo "budget_seconds: $BUDGET"
  echo "case_timeout_seconds: $CASE_TIMEOUT"
  echo "layout_error_baseline: $LAYOUT_BASELINE"
} > "$OUT/environment.txt"

{
  echo "# sha256 замороженных входов (config/artifacts)"
  for f in Cargo.toml Cargo.lock rust-toolchain.toml \
           docs/specs/STORAGE_FORMAT.md docs/plans/STAGE_9.md \
           tools/stage9/run_checks.sh tools/check_layout.py; do
    printf '%s  %s\n' "$(hash_of "$f")" "$f"
  done
} > "$OUT/frozen.txt"

printf 'case\tcommand\tstatus\texit_code\tseconds\tevidence\tnote\n' > "$RESULTS"

# --- Хелперы -----------------------------------------------------------------
record() { # case command status exit seconds evidence note
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" "$6" "$7" >> "$RESULTS"
  printf '[%s] %-28s %s\n' "$3" "$1" "$7"
}

have() { command -v "$1" >/dev/null 2>&1; }

# run_with_timeout SECONDS cmd...  (macOS без coreutils timeout -> perl alarm)
run_with_timeout() {
  local secs="$1"; shift
  if have perl; then
    perl -e 'alarm shift; exec @ARGV or exit 127' "$secs" "$@"
  else
    "$@"
  fi
}

# run_case NAME "display command" PREREQ_CMD -- cmd args...
# PREREQ_CMD: строка для sh -c; ненулевой код означает "инструмент недоступен".
run_case() {
  local name="$1" display="$2" prereq="$3"; shift 4
  local log="logs/$name.log"
  local now elapsed
  now=$(date +%s); elapsed=$((now - START_EPOCH))
  if [ "$elapsed" -ge "$BUDGET" ]; then
    record "$name" "$display" not_run - 0 - "бюджет ${BUDGET}s исчерпан до запуска; не выполнено"
    return
  fi
  if ! sh -c "$prereq" >/dev/null 2>&1; then
    record "$name" "$display" not_run - 0 - "инструмент недоступен (проверка: $prereq)"
    return
  fi
  local t0 t1 rc
  t0=$(date +%s)
  run_with_timeout "$CASE_TIMEOUT" "$@" > "$OUT/$log" 2>&1
  rc=$?
  t1=$(date +%s)
  if [ "$rc" -eq 0 ]; then
    record "$name" "$display" passed 0 $((t1 - t0)) "$log" ""
  elif [ "$rc" -eq 142 ]; then
    record "$name" "$display" failed "$rc" $((t1 - t0)) "$log" "таймаут ${CASE_TIMEOUT}s (SIGALRM)"
  else
    record "$name" "$display" failed "$rc" $((t1 - t0)) "$log" ""
  fi
}

# --- Исполняемые case --------------------------------------------------------
run_case fmt_check "cargo fmt --all -- --check" "cargo fmt --version" -- \
  cargo fmt --all -- --check
run_case od_storage "cargo test -p od-storage" "cargo --version" -- \
  cargo test -p od-storage
run_case od_host_storage_policy "cargo test -p od-host --lib storage_policy" "cargo --version" -- \
  cargo test -p od-host --lib storage_policy
run_case od_host_storage_io "cargo test -p od-host --lib storage_io" "cargo --version" -- \
  cargo test -p od-host --lib storage_io
run_case od_host_raw_file_tests "cargo test -p od-host --lib raw_file_tests" "cargo --version" -- \
  cargo test -p od-host --lib raw_file_tests
run_case od_host_persistent_nvme "cargo test -p od-host --lib persistent_nvme" "cargo --version" -- \
  cargo test -p od-host --lib persistent_nvme
run_case od_host_stage9_storage_kill "cargo test -p od-host --test stage9_storage_kill" \
  "[ \"\$(uname -s)\" = Darwin ]" -- \
  cargo test -p od-host --test stage9_storage_kill
run_case od_image_tool "cargo test -p od-image-tool" "cargo --version" -- \
  cargo test -p od-image-tool
run_case workspace_check "cargo check --workspace --all-targets --offline" "cargo --version" -- \
  cargo check --workspace --all-targets --offline
run_case git_diff_check "git diff --check" "git --version" -- \
  git diff --check

# S9-40: один прогон бенчмарка хранилища (воспроизводимость кода замера и сверки
# с моделью). Это не замер: цифры одного прогона не публикуются, полный набор —
# docs/testing/S9_40_CHAIN_EXPORT_COMPACT.md. Только macOS/aarch64.
mkdir -p "$OUT/s9_40_bench"
run_case s9_40_bench_once "s9_40_storage <empty-dir> 1 (release)" \
  "[ \"\$(uname -s)-\$(uname -m)\" = Darwin-arm64 ]" -- \
  bash -c 'cargo build -p od-bench --release --offline --bin s9_40_storage && "$1/target/release/s9_40_storage" "$2" 1' \
  _ "$ROOT" "$OUT/s9_40_bench"

# check_layout: известное число ERROR (ссылки на work/, которых нет в чистом worktree)
# не должно расти. Ненулевой код выхода при ERROR <= baseline ожидаем, не провал.
layout_case() {
  local name=layout_check display="python3 tools/check_layout.py"
  local now elapsed
  now=$(date +%s); elapsed=$((now - START_EPOCH))
  if [ "$elapsed" -ge "$BUDGET" ]; then
    record "$name" "$display" not_run - 0 - "бюджет ${BUDGET}s исчерпан до запуска; не выполнено"; return
  fi
  if ! have python3; then
    record "$name" "$display" not_run - 0 - "python3 недоступен"; return
  fi
  local t0 t1 rc n
  t0=$(date +%s)
  python3 tools/check_layout.py > "$OUT/logs/$name.log" 2>&1
  rc=$?
  t1=$(date +%s)
  n=$(grep -c '^ERROR' "$OUT/logs/$name.log")
  if [ "$rc" -eq 0 ]; then
    record "$name" "$display" passed "$rc" $((t1 - t0)) "logs/$name.log" "ERROR=0"
  elif [ "$n" -le "$LAYOUT_BASELINE" ] && [ "$n" -gt 0 ]; then
    record "$name" "$display" passed "$rc" $((t1 - t0)) "logs/$name.log" \
      "известный baseline: ERROR=$n <= $LAYOUT_BASELINE, не растёт; чистой структурой не считать"
  else
    record "$name" "$display" failed "$rc" $((t1 - t0)) "logs/$name.log" \
      "ERROR=$n, baseline=$LAYOUT_BASELINE (рост или сбой запуска)"
  fi
}
layout_case

# --- Статичные case: не выполняются этим скриптом ----------------------------
# Локальный успех выше НЕ заполняет эти строки.
record S9-38_vm_matrix       "(VM fault matrix)"        blocked - 0 docs/reports/STAGE_9_REPORT.md "VM-часть S9-38 заблокирована S9-37, S9-29…31, S9-13"
record S9-39_power_loss      "(стенд power-loss)"       blocked - 0 docs/testing/S9_39_POWER_LOSS_PLAN.md "вне скрипта: процедура принята, 15 прогонов verify ok (§7), N сокращено с 20 до 5 на стадию; приём результатов не выполнен, точки между барьерами 1 и 2 не покрыты"
record windows_vtpm_uefi     "(Windows/UEFI/vTPM)"      blocked - 0 docs/reports/STAGE_9_REPORT.md "нет Windows guest, UEFI variables store (этап 7, S7-09/S7-10) и vTPM (S9-26/27/28)"
record linux_mini_guest      "(Linux Mini guest)"       blocked - 0 docs/reports/STAGE_9_REPORT.md "реальный Linux guest на диске этапа 9 не запускался: образ гостю не подключён (S9-13 blocked)"
record real_guest_lifecycle  "(real guest, lifecycle)"  blocked - 0 docs/KNOWN_ISSUES.md "S9-28…S9-37 заблокированы компонентами этапов 4-8 и ISSUE-056"
record S9-22_migration       "(версии формата)"         not_run - 0 docs/specs/STORAGE_FORMAT.md "закрыта в объёме v1 (решение владельца 2026-11-18): RAW -> CoW v1 через import покрыт od_storage и od_image_tool (§9.7); миграции между версиями CoW нет (второй версии нет)"
record S9-40_full            "(checkpoint/restore замеры)" blocked - 0 docs/testing/S9_40_CHAIN_EXPORT_COMPACT.md "partial: storage-замеры сделаны (docs/testing/S9_40_*.md); пауза/длительность checkpoint и restore заблокированы S9-29…31; not_run: фрагментация, реальные размеры диска, независимый повтор"
record S9-43_independent_review "(независимое ревью)"   blocked - 0 docs/reports/STAGE_9_REPORT.md "вне скрипта: ревью storage-scope выполнено (Claude Opus high, засчитано владельцем 2026-11-18, отчёт §10); ревью полной VM невозможно — S9-38, S9-39, S9-23…37 не закрыты; скрипт passed не ставит"

# --- Сводка ------------------------------------------------------------------
END_EPOCH=$(date +%s)
{
  echo "run_id: $RUN_ID"
  echo "total_seconds: $((END_EPOCH - START_EPOCH))"
  for s in passed failed not_run blocked; do
    printf '%s: %s\n' "$s" "$(awk -F'\t' -v s="$s" 'NR>1 && $3==s' "$RESULTS" | wc -l | tr -d ' ')"
  done
  echo "stage9_accepted: no (blocked/not_run gates остаются видимыми; см. docs/testing/STAGE_9_REPRO.md)"
} > "$OUT/summary.txt"

echo "----"
cat "$OUT/summary.txt"
echo "results: $RESULTS"

if awk -F'\t' 'NR>1 && $3=="failed"{f=1} END{exit f?0:1}' "$RESULTS"; then
  exit 1
fi
exit 0
