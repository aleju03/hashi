#!/bin/sh
# linux keeps a program's memory to itself and its parent: this lets hashi-reader read osu!'s
PATH="$PATH:/usr/sbin:/sbin"
if command -v setcap >/dev/null 2>&1; then
  setcap cap_sys_ptrace=eip /usr/bin/hashi-reader || true
fi
