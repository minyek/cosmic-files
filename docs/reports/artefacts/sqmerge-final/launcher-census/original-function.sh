live_job_count() {
    local c d s n=0
    while read -r c; do
        for d in "$c"/*/; do
            [[ -d "$d" ]] || continue
            if [[ "${d%/}" == *.slice ]]; then
                for s in "$d"*/; do [[ -d "$s" ]] && n=$(( n + 1 )); done
            else
                n=$(( n + 1 ))
            fi
        done
    done < <(jobs_compartments)
    printf '%s\n' "$n"
}
