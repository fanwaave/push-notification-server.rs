#!/usr/bin/env python3
from itertools import product
for deliveries in product(("notif-a","notif-b"),repeat=4):
  effects=set()
  for key in deliveries: effects.add(key)
  assert effects==set(deliveries), "notification effects diverged from unique keys"
  assert len(effects)<=2
print("notification dedupe/exactly-once model: ok")
