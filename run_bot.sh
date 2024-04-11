#!/bin/bash

./github-pr-mirror-bot > >(tee -a mirror_bot_stdout_$(date +%Y-%m-%d).log) 2> >(tee -a mirror_bot_stderr_$(date +%Y-%m-%d).log >&2)