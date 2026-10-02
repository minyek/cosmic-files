live_job_count() {
    local c d n=0
    local -a pending=()
    while read -r c; do pending+=("$c"); done < <(jobs_compartments)
    while (( ${#pending[@]} > 0 )); do
        c="${pending[-1]}"
        unset 'pending[-1]'
        for d in "$c"/*/; do
            [[ -d "$d" ]] || continue
            if [[ "${d%/}" == *.slice ]]; then
                pending+=("${d%/}")
            elif awk '$1 == "populated" && $2 == 1 { live=1 } END { exit !live }' \
                "$d/cgroup.events" 2>/dev/null; then
                n=$(( n + 1 ))
            fi
        done
    done
    printf '%s\n' "$n"
}
