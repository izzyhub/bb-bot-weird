#!/usr/bin/env bash

esptool.py --chip esp32s3 elf2image --flash_mode dio --flash_size 16MB target/xtensa-esp32s3-none-elf/release/bb-bot-weird
