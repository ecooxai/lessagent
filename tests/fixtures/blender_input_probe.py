"""Observe a disposable Blender GUI for the macOS input regression.

Run with --factory-startup --no-window-focus --python THIS -- STATE_JSON.
The observer only records events and scene/UI state; it never generates input,
renames/transforms/deletes objects, saves user preferences, or opens user files.
"""
import json
import os
from pathlib import Path
import sys
import time

import bpy

STATE = Path(sys.argv[sys.argv.index('--') + 1])
EVENTS = []
SEQ = 0
bpy.context.preferences.view.show_splash = False


class LESSAGENT_OT_input_probe(bpy.types.Operator):
    bl_idname = 'wm.lessagent_input_probe'
    bl_label = 'Lessagent read-only input observer'

    def modal(self, context, event):
        global SEQ
        if event.type not in {'TIMER', 'TIMER_REPORT', 'INBETWEEN_MOUSEMOVE'}:
            SEQ += 1
            EVENTS.append(dict(seq=SEQ, type=event.type, value=event.value,
                               x=event.mouse_x, y=event.mouse_y,
                               shift=event.shift, ctrl=event.ctrl,
                               alt=event.alt, oskey=event.oskey,
                               text=event.unicode))
            del EVENTS[:-500]
        return {'PASS_THROUGH'}

    def invoke(self, context, event):
        context.window_manager.modal_handler_add(self)
        return {'RUNNING_MODAL'}


def snapshot():
    value = dict(pid=os.getpid(), time=time.monotonic(), seq=SEQ, events=list(EVENTS),
                 objects=[dict(name=o.name, selected=o.select_get(),
                               location=list(o.location)) for o in bpy.context.scene.objects],
                 windows=[])
    for w in bpy.context.window_manager.windows:
        areas = []
        for a in w.screen.areas:
            item = dict(type=a.type, x=a.x, y=a.y, width=a.width, height=a.height,
                        regions=[dict(type=r.type, x=r.x, y=r.y, width=r.width,
                                      height=r.height) for r in a.regions])
            if a.type == 'VIEW_3D':
                item['sidebar'] = a.spaces.active.show_region_ui
                item['view_distance'] = a.spaces.active.region_3d.view_distance
                item['view_rotation'] = list(a.spaces.active.region_3d.view_rotation)
            areas.append(item)
        value['windows'].append(dict(width=w.width, height=w.height,
                                    pixel_size=bpy.context.preferences.system.pixel_size, areas=areas))
    tmp = STATE.with_suffix('.tmp')
    tmp.write_text(json.dumps(value))
    tmp.replace(STATE)
    return 0.1


def begin():
    if not bpy.context.window_manager.windows:
        return 0.2
    bpy.utils.register_class(LESSAGENT_OT_input_probe)
    window = bpy.context.window_manager.windows[0]
    with bpy.context.temp_override(window=window):
        bpy.ops.wm.lessagent_input_probe('INVOKE_DEFAULT')
    bpy.app.timers.register(snapshot, first_interval=0.1)


bpy.app.timers.register(begin, first_interval=1.0)
