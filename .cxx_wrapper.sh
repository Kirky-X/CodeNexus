#!/bin/bash
exec /usr/bin/c++ -fuse-ld=gold "$@"
