"""Read-only observer installed via Blender's console in owned GUI tests.

STATE_PATH is supplied by the test. Never edits objects or user preferences.
"""
import json
import os
from pathlib import Path
import time
import bpy

_STATE = Path(STATE_PATH)
_EVENTS = []
_SEQ = 0

class LESSAGENT_OT_model_probe(bpy.types.Operator):
    bl_idname = 'wm.lessagent_model_probe'
    bl_label = 'Read-only Lessagent model observer'

    def modal(self, context, event):
        global _SEQ
        if event.type not in {'TIMER', 'TIMER_REPORT', 'INBETWEEN_MOUSEMOVE'}:
            _SEQ += 1
            _EVENTS.append(dict(seq=_SEQ, type=event.type, value=event.value,
                               x=event.mouse_x, y=event.mouse_y, text=event.unicode,
                               shift=event.shift, ctrl=event.ctrl, alt=event.alt, oskey=event.oskey))
            del _EVENTS[:-2000]
        return {'PASS_THROUGH'}

    def invoke(self, context, event):
        context.window_manager.modal_handler_add(self)
        return {'RUNNING_MODAL'}

def snapshot():
    value = dict(pid=os.getpid(), time=time.monotonic(), seq=_SEQ, events=list(_EVENTS),
                 active=bpy.context.view_layer.objects.active.name if bpy.context.view_layer.objects.active else None,
                 objects=[dict(name=o.name, type=o.type, selected=o.select_get(), mode=o.mode,
                               location=list(o.location), scale=list(o.scale), rotation=list(o.rotation_euler),
                               vertices=len(o.data.vertices) if o.type == 'MESH' else 0,
                               faces=len(o.data.polygons) if o.type == 'MESH' else 0)
                          for o in bpy.context.scene.objects], windows=[])
    for w in bpy.context.window_manager.windows:
        areas = []
        for a in w.screen.areas:
            item = dict(type=a.type, x=a.x, y=a.y, width=a.width, height=a.height,
                        regions=[dict(type=r.type,x=r.x,y=r.y,width=r.width,height=r.height) for r in a.regions])
            if a.type == 'VIEW_3D':
                r=a.spaces.active.region_3d
                item['view']=dict(rotation=list(r.view_rotation),location=list(r.view_location),distance=r.view_distance)
            if a.type == 'CONSOLE':
                item['history'] = [h.body for h in a.spaces.active.history][-3:]
            areas.append(item)
        value['windows'].append(dict(width=w.width, height=w.height,
                                    pixel_size=bpy.context.preferences.system.pixel_size, areas=areas))
    temp = _STATE.with_suffix('.tmp'); temp.write_text(json.dumps(value)); temp.replace(_STATE)
    return 0.1

bpy.utils.register_class(LESSAGENT_OT_model_probe)
bpy.ops.wm.lessagent_model_probe('INVOKE_DEFAULT')
bpy.app.timers.register(snapshot, first_interval=0.1)
