"""Ronnie 3000 pedal skins, rendered in Blender.

Run: Blender -b -P pedals.py -- <family> <out_dir> [samples]
Families: od-green, od-gold, dist-black, fuzz, boost, comp, default-dark, default-light
Units: 1 BU = 1 cm. Shared camera, lights and scale across every family.
"""
import math
import os
import sys

import bmesh
import bpy
from mathutils import Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import finish, knob_rig, lcd_rig  # noqa: E402

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
FAMILY = argv[0] if argv else "od-green"
OUT = argv[1] if len(argv) > 1 else os.path.join(os.getcwd(), FAMILY)
SAMPLES = int(argv[2]) if len(argv) > 2 else 256
WORLD_ROT = 320

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.unit_settings.system = "METRIC"
scene.unit_settings.scale_length = 0.01

# ================================================================ materials
def principled(name, color, metallic=0.0, rough=0.4, coat=0.0, coat_rough=0.05,
               aniso=0.0, bump=None, flake=None):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    p = nt.nodes["Principled BSDF"]
    p.inputs["Base Color"].default_value = (*color, 1)
    p.inputs["Metallic"].default_value = metallic
    p.inputs["Roughness"].default_value = rough
    for key, val in (("Coat Weight", coat), ("Coat Roughness", coat_rough), ("Anisotropic", aniso)):
        if key in p.inputs:
            p.inputs[key].default_value = val
    normal_in = None
    if bump:
        scale, strength = bump
        tex = nt.nodes.new("ShaderNodeTexNoise")
        tex.inputs["Scale"].default_value = scale
        tex.inputs["Detail"].default_value = 8
        bn = nt.nodes.new("ShaderNodeBump")
        bn.inputs["Strength"].default_value = strength
        bn.inputs["Distance"].default_value = 0.02
        nt.links.new(tex.outputs["Fac"], bn.inputs["Height"])
        nt.links.new(bn.outputs["Normal"], p.inputs["Normal"])
        normal_in = bn
    if flake:
        # metal-flake sparkle: tiny voronoi cells vary roughness and tilt the normal
        scale, strength = flake
        vor = nt.nodes.new("ShaderNodeTexVoronoi")
        vor.inputs["Scale"].default_value = scale
        ramp = nt.nodes.new("ShaderNodeMapRange")
        ramp.inputs["To Min"].default_value = rough * 0.4
        ramp.inputs["To Max"].default_value = rough * 1.4
        nt.links.new(vor.outputs["Distance"], ramp.inputs["Value"])
        nt.links.new(ramp.outputs["Result"], p.inputs["Roughness"])
        fb = nt.nodes.new("ShaderNodeBump")
        fb.inputs["Strength"].default_value = strength
        fb.inputs["Distance"].default_value = 0.01
        nt.links.new(vor.outputs["Color"], fb.inputs["Height"])
        if normal_in:
            nt.links.new(normal_in.outputs["Normal"], fb.inputs["Normal"])
        nt.links.new(fb.outputs["Normal"], p.inputs["Normal"])
    return m


def emission_only(name, color, strength):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nt = m.node_tree
    for n in list(nt.nodes):
        nt.nodes.remove(n)
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    em = nt.nodes.new("ShaderNodeEmission")
    em.inputs["Color"].default_value = (*color, 1)
    em.inputs["Strength"].default_value = strength
    nt.links.new(em.outputs["Emission"], out.inputs["Surface"])
    return m


def add_edge_wear(mat, wear_color, lo=0.52, hi=0.62):
    nt = mat.node_tree
    p = nt.nodes["Principled BSDF"]
    if p.inputs["Base Color"].is_linked:
        return
    base = tuple(p.inputs["Base Color"].default_value)
    geo = nt.nodes.new("ShaderNodeNewGeometry")
    ramp = nt.nodes.new("ShaderNodeValToRGB")
    ramp.color_ramp.elements[0].position = lo
    ramp.color_ramp.elements[1].position = hi
    noise = nt.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 60
    mult = nt.nodes.new("ShaderNodeMath")
    mult.operation = "MULTIPLY"
    nt.links.new(geo.outputs["Pointiness"], ramp.inputs["Fac"])
    nt.links.new(ramp.outputs["Color"], mult.inputs[0])
    nt.links.new(noise.outputs["Fac"], mult.inputs[1])
    mix = nt.nodes.new("ShaderNodeMix")
    mix.data_type = "RGBA"
    mix.inputs["A"].default_value = base
    mix.inputs["B"].default_value = (*wear_color, 1)
    nt.links.new(mult.outputs["Value"], mix.inputs["Factor"])
    nt.links.new(mix.outputs["Result"], p.inputs["Base Color"])


M_RUBBER = principled("rubber", (0.018, 0.018, 0.018), rough=0.75, bump=(400, 0.15))
M_CHROME = principled("chrome", (0.9, 0.9, 0.9), metallic=1.0, rough=0.08)
M_NICKEL = principled("nickel", (0.75, 0.74, 0.72), metallic=1.0, rough=0.22)
M_JACK = principled("jack black", (0.02, 0.02, 0.02), rough=0.45)
M_BASE = principled("base plate", (0.018, 0.018, 0.018), metallic=0.4, rough=0.5)
M_GLASS = principled("lcd glass", (0.004, 0.006, 0.005), rough=0.25)
KNOBS = {
    "black": principled("knob black", (0.012, 0.012, 0.012), rough=0.38, coat=0.6, coat_rough=0.12),
    "chrome": principled("knob chrome", (0.85, 0.85, 0.85), metallic=1.0, rough=0.12),
    "cream": principled("knob cream", (0.72, 0.66, 0.52), rough=0.35, coat=0.5, coat_rough=0.15),
}

# ================================================================ families
F = {
    "od-green": dict(form="treadle", name="GREEN OD", paint=dict(color=(0.016, 0.15, 0.05), rough=0.45, coat=0.22, coat_rough=0.28, bump=(1600, 0.05)),
                     wear=(0.10, 0.36, 0.16), ink=(0.86, 0.85, 0.80), knob="black", led=(1.0, 0.025, 0.0)),
    "od-gold": dict(form="standard", name="GOLD OD", paint=dict(color=(0.42, 0.24, 0.045), metallic=0.9, rough=0.3, coat=0.6, coat_rough=0.08, flake=(900, 0.3)),
                    wear=(0.8, 0.62, 0.3), ink=(0.05, 0.035, 0.015), knob="black", led=(1.0, 0.025, 0.0), frame=True, track=0.25),
    "dist-black": dict(form="row3", name="DIST", paint=dict(color=(0.012, 0.012, 0.013), rough=0.72, bump=(2500, 0.12)),
                       wear=(0.08, 0.08, 0.08), ink=(0.85, 0.85, 0.85), knob="chrome", led=(1.0, 0.025, 0.0), bold=0.03),
    "fuzz": dict(form="wide", name="FUZZ", paint=dict(color=(0.62, 0.63, 0.64), metallic=1.0, rough=0.3, aniso=0.7),
                 wear=None, ink=(0.45, 0.025, 0.015), knob="black", led=(1.0, 0.025, 0.0), shear=0.2, bold=0.03, frame=True),
    "boost": dict(form="compact", name="BOOST", paint=dict(color=(0.74, 0.71, 0.62), rough=0.55, bump=(2200, 0.06)),
                  wear=(0.55, 0.53, 0.47), ink=(0.06, 0.06, 0.06), knob="black", led=(1.0, 0.025, 0.0)),
    "comp": dict(form="compact", name="COMP", paint=dict(color=(0.42, 0.02, 0.012), rough=0.38, coat=0.4, coat_rough=0.15, bump=(1600, 0.05)),
                 wear=(0.6, 0.15, 0.1), ink=(0.9, 0.86, 0.78), knob="black", led=(1.0, 0.025, 0.0), bold=0.02),
    "default-dark": dict(form="standard", name="MYSTERY", paint=dict(color=(0.09, 0.09, 0.1), metallic=0.7, rough=0.34, aniso=0.6),
                         wear=None, ink=(0.88, 0.88, 0.86), knob="black", led=(1.0, 0.18, 0.0)),
    "default-light": dict(form="standard", name="MY PEDAL", paint=dict(color=(0.6, 0.61, 0.62), metallic=1.0, rough=0.3, aniso=0.6),
                          wear=None, ink=(0.04, 0.04, 0.045), knob="black", led=(1.0, 0.18, 0.0)),
}
cfg = F[FAMILY]
M_PAINT = principled("paint", **cfg["paint"])
if cfg.get("wear"):
    add_edge_wear(M_PAINT, cfg["wear"])
M_INK = principled("ink", cfg["ink"], rough=0.5)
M_LED = emission_only("led", cfg["led"], 2.6)
M_KNOB = KNOBS[cfg["knob"]]

# ================================================================ helpers
def assign(obj, mat):
    obj.data.materials.clear()
    obj.data.materials.append(mat)


def smooth(obj):
    for poly in obj.data.polygons:
        poly.use_smooth = True


def bevel(obj, width, segments=6, harden=True):
    mod = obj.modifiers.new("bevel", "BEVEL")
    mod.width = width
    mod.segments = segments
    mod.limit_method = "ANGLE"
    try:
        mod.harden_normals = harden
    except AttributeError:
        pass
    smooth(obj)


def box(name, size, loc, mat, bev=0.0, seg=6):
    bpy.ops.mesh.primitive_cube_add(size=1, location=loc)
    o = bpy.context.active_object
    o.name = name
    o.scale = size
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    if bev:
        bevel(o, bev, seg)
    assign(o, mat)
    return o


def cyl(name, r, depth, loc, mat, verts=64, rot=(0, 0, 0), bev=0.0):
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=r, depth=depth, location=loc, rotation=rot)
    o = bpy.context.active_object
    o.name = name
    if bev:
        bevel(o, bev, 4)
    assign(o, mat)
    return o


def load_font(paths):
    for p in paths:
        if os.path.exists(p):
            try:
                return bpy.data.fonts.load(p)
            except Exception:
                pass
    return None


FONT = load_font(["/System/Library/Fonts/Supplemental/Futura.ttc", "/System/Library/Fonts/Avenir Next.ttc"])


def text(body, size, loc, mat, extrude=0.0, font=FONT, shear=0.0, bold=0.0):
    curve = bpy.data.curves.new(type="FONT", name=body)
    curve.body = body
    curve.size = size
    curve.align_x = "CENTER"
    curve.align_y = "CENTER"
    curve.extrude = extrude
    curve.shear = shear
    curve.offset = bold
    if font:
        curve.font = font
    o = bpy.data.objects.new("txt " + body, curve)
    bpy.context.collection.objects.link(o)
    o.location = loc
    o.data.materials.append(mat)
    return o

# ================================================================ parts
def enclosure(W, D, TOP, radius=0.5):
    box("enclosure", (W, D, TOP - 0.85), (0, 0, 0.85 + (TOP - 0.85) / 2), M_PAINT, bev=radius, seg=10)
    box("base plate", (W - 0.12, D - 0.12, 1.0), (0, 0, 0.5), M_BASE, bev=0.28, seg=6)


def knob(name, x, y, r, h, value, top, label=None, label_below=False):
    bpy.ops.mesh.primitive_cylinder_add(vertices=72, radius=r, depth=h * 0.72, location=(x, y, top + h * 0.36))
    skirt = bpy.context.active_object
    skirt.name = name + " skirt"
    me = skirt.data
    bm = bmesh.new()
    bm.from_mesh(me)
    step = 2 * math.pi / 72
    for v in bm.verts:
        if round(math.atan2(v.co.y, v.co.x) / step) % 2:
            v.co.x *= 0.935
            v.co.y *= 0.935
    bm.to_mesh(me)
    bm.free()
    bevel(skirt, 0.03, 2, harden=False)
    assign(skirt, M_KNOB)
    cyl(name + " cap", r * 0.86, h * 0.34, (x, y, top + h * 0.87), M_KNOB, verts=96, bev=0.12)
    ang = math.radians(225 - 270 * value)
    ptr_mat = KNOBS["black"] if cfg["knob"] != "black" else M_POINTER
    ptr = box(name + " pointer", (0.07, r * 0.55, 0.03), (x + math.cos(ang) * r * 0.42, y + math.sin(ang) * r * 0.42, top + h * 1.04 + 0.015), ptr_mat)
    ptr.rotation_euler = (0, 0, ang - math.pi / 2)
    if label:
        knob_rig.add(label, (x, y), (x, y, top + h * 1.04), r, (0, 0, 1), [(ptr, r * 0.42, None)])
    for i in range(11):
        a = math.radians(225 - 27 * i)
        cyl(name + f" dot{i}", 0.035, 0.01, (x + math.cos(a) * r * 1.28, y + math.sin(a) * r * 1.28, top + 0.004), M_INK, verts=12)
    if label:
        ly = y - r * 1.55 if label_below else y + r * 1.6
        text(label, 0.3, (x, ly, top + 0.004), M_INK)


M_POINTER = principled("pointer", (0.88, 0.87, 0.84), rough=0.5)


def led(x, y, top, label=None):
    bpy.ops.mesh.primitive_torus_add(major_radius=0.2, minor_radius=0.06, location=(x, y, top + 0.05))
    bz = bpy.context.active_object
    smooth(bz)
    assign(bz, M_CHROME)
    bpy.ops.mesh.primitive_uv_sphere_add(radius=0.15, location=(x, y, top + 0.06))
    l = bpy.context.active_object
    smooth(l)
    assign(l, M_LED)
    glow = bpy.data.lights.new("led glow", "POINT")
    glow.energy = 3.0
    glow.color = cfg["led"]
    glow.shadow_soft_size = 0.1
    g = bpy.data.objects.new("led glow", glow)
    g.location = (x, y, top + 0.35)
    bpy.context.collection.objects.link(g)
    if label:
        text(label, 0.2, (x, y - 0.5, top + 0.004), M_INK)


def lcd(x, y, top, w=2.05, h=0.82):
    """A blank LCD: bezel and dark glass. The app draws the text; the manifest records the glass."""
    box("lcd bezel", (w, h, 0.08), (x, y, top + 0.05), M_JACK, bev=0.08, seg=3)
    gw, gh, gd, gz = w - 0.2, h - 0.18, 0.04, top + 0.1
    box("lcd glass", (gw, gh, gd), (x, y, gz), M_GLASS, bev=0.03, seg=2)
    lcd_rig.set_glass(x, y, gz + gd / 2, gw, gh)


def toggle(x, y, top, up_label, down_label=None, up=True):
    cyl("toggle nut", 0.3, 0.14, (x, y, top + 0.07), M_NICKEL, verts=6, bev=0.02)
    cyl("toggle washer", 0.2, 0.1, (x, y, top + 0.18), M_NICKEL, verts=32, bev=0.02)
    tilt = math.radians(-22 if up else 22)
    lever = cyl("toggle bat", 0.065, 0.75, (x, y + math.sin(-tilt) * 0.3, top + 0.5), M_CHROME, verts=24, rot=(tilt, 0, 0))
    bpy.ops.mesh.primitive_uv_sphere_add(radius=0.09, location=(x, y + math.sin(-tilt) * 0.66, top + 0.83))
    tip = bpy.context.active_object
    smooth(tip)
    assign(tip, M_CHROME)
    text(up_label, 0.22, (x, y + 0.62, top + 0.004), M_INK)
    if down_label:
        text(down_label, 0.22, (x, y - 0.62, top + 0.004), M_INK)


def footswitch(x, y, top):
    cyl("fs nut", 0.85, 0.25, (x, y, top + 0.125), M_NICKEL, verts=6, bev=0.04)
    cyl("fs collar", 0.6, 0.38, (x, y, top + 0.43), M_NICKEL, verts=64, bev=0.05)
    cyl("fs cap", 0.5, 0.32, (x, y, top + 0.75), M_CHROME, verts=64, bev=0.16)


def arrows(xl, xr, y, top):
    for xx in (xl, xr):
        bpy.ops.mesh.primitive_circle_add(vertices=3, radius=0.17, fill_type="NGON", location=(xx, y, top + 0.004), rotation=(0, 0, math.radians(-90)))
        tri = bpy.context.active_object
        assign(tri, M_INK)


def jacks(W, y, z):
    for side in (-1, 1):
        x = side * (W / 2 + 0.05)
        cyl("jack body", 0.42, 0.5, (x + side * 0.2, y, z), M_JACK, verts=32, rot=(0, math.radians(90), 0), bev=0.04)
        cyl("hex nut", 0.62, 0.22, (x + side * 0.08, y, z), M_NICKEL, verts=6, rot=(0, math.radians(90), 0), bev=0.03)
        cyl("jack sleeve", 0.3, 0.6, (x + side * 0.45, y, z), M_NICKEL, verts=48, rot=(0, math.radians(90), 0), bev=0.02)


def frame_line(W, D, top, inset=0.45):
    t = 0.035
    for sx, sy, lx, ly in ((W - 2 * inset, t, 0, D / 2 - inset), (W - 2 * inset, t, 0, -D / 2 + inset),
                           (t, D - 2 * inset, W / 2 - inset, 0), (t, D - 2 * inset, -W / 2 + inset, 0)):
        box("frame", (sx, sy, 0.006), (lx, ly, top + 0.003), M_INK)


def name_text(y, size, top):
    text(cfg["name"], size, (0, y, top + 0.004), M_INK, extrude=0.004, shear=cfg.get("shear", 0.0), bold=cfg.get("bold", 0.0))


def nine_volt(x, y, top):
    text("9VDC", 0.18, (x, y, top + 0.004), M_INK)

# ================================================================ forms
form = cfg["form"]
if form == "treadle":
    W, D, TOP = 7.4, 12.5, 4.0
    enclosure(W, D, TOP, 0.5)
    box("label strip", (W - 1.1, 1.55, 0.04), (0, 0.55, TOP + 0.005), principled("inset", tuple(c * 0.5 for c in cfg["paint"]["color"]), rough=0.5), bev=0.12, seg=4)
    text(cfg["name"], 0.95, (-1.15, 0.55, TOP + 0.03), M_INK, extrude=0.006)
    lcd(1.95, 0.55, TOP)
    knob("gain", -2.05, 3.75, 0.82, 1.15, 0.45, TOP, "GAIN IN")
    knob("output", 2.05, 3.75, 0.82, 1.15, 0.6, TOP, "OUTPUT")
    knob("mix", 0, 2.35, 0.52, 0.9, 1.0, TOP, "MIX", label_below=True)
    led(0, 4.55, TOP, "CHECK")
    pivot = bpy.data.objects.new("treadle pivot", None)
    bpy.context.collection.objects.link(pivot)
    pivot.location = (0, -0.35, TOP)
    parts = [box("treadle plate", (W - 0.5, 5.75, 0.7), (0, -3.25, TOP + 0.4), M_PAINT, bev=0.28, seg=8),
             box("rubber pad", (W - 1.3, 5.1, 0.12), (0, -3.3, TOP + 0.8), M_RUBBER, bev=0.05, seg=3)]
    for i in range(15):
        parts.append(box(f"rib {i}", (W - 1.55, 0.15, 0.12), (0, -5.55 + i * 0.32, TOP + 0.9), M_RUBBER, bev=0.05, seg=3))
    for o in parts:
        o.parent = pivot
        o.location -= pivot.location
    pivot.rotation_euler = (math.radians(-2.5), 0, 0)
    box("hinge gap", (W - 0.6, 0.1, 0.05), (0, -0.33, TOP + 0.01), M_JACK)
    jacks(W, 1.4, 2.2)

elif form == "standard":
    W, D, TOP = 7.4, 12.0, 4.0
    enclosure(W, D, TOP, 0.55)
    knob("gain", -2.2, 3.55, 0.85, 1.2, 0.55, TOP, "GAIN IN")
    knob("output", 2.2, 3.55, 0.85, 1.2, 0.5, TOP, "OUTPUT")
    text("CAPTURE", 0.2, (0, 4.75, TOP + 0.004), M_INK)
    lcd(0, 4.1, TOP, w=1.9, h=0.72)
    knob("mix", 0, 1.75, 0.6, 1.0, 1.0, TOP, "MIX", label_below=True)
    toggle(-2.15, 1.75, TOP, "NORM", "ON")
    toggle(2.15, 1.75, TOP, "FULL", "LITE")
    led(0, -0.05, TOP)
    name_text(-1.45, 1.0 if len(cfg["name"]) <= 7 else 0.82, TOP)
    arrows(-3.0, 3.0, -2.35, TOP)
    footswitch(0, -3.85, TOP)
    nine_volt(2.65, -5.1, TOP)
    if cfg.get("frame"):
        frame_line(W, D, TOP)
    jacks(W, -0.6, 2.2)

elif form == "row3":
    W, D, TOP = 7.6, 12.0, 4.0
    enclosure(W, D, TOP, 0.5)
    knob("output", -2.45, 3.9, 0.76, 1.1, 0.5, TOP, "OUTPUT")
    knob("mix", 0, 3.9, 0.76, 1.1, 1.0, TOP, "MIX")
    knob("gain", 2.45, 3.9, 0.76, 1.1, 0.7, TOP, "GAIN IN")
    lcd(0, 2.2, TOP, w=1.9, h=0.72)
    toggle(-2.3, 2.2, TOP, "NORM")
    toggle(2.3, 2.2, TOP, "FULL")
    led(0, 0.9, TOP)
    name_text(-0.6, 1.15, TOP)
    arrows(-3.1, 3.1, -1.6, TOP)
    footswitch(0, -3.7, TOP)
    nine_volt(2.7, -5.1, TOP)
    jacks(W, -0.4, 2.2)

elif form == "wide":
    W, D, TOP = 10.2, 12.8, 4.1
    enclosure(W, D, TOP, 0.55)
    knob("output", -3.2, 4.1, 0.95, 1.25, 0.55, TOP, "OUTPUT")
    knob("mix", 0, 4.1, 0.95, 1.25, 1.0, TOP, "MIX")
    knob("gain", 3.2, 4.1, 0.95, 1.25, 0.75, TOP, "GAIN IN")
    lcd(0, 2.15, TOP, w=2.0, h=0.75)
    toggle(-2.6, 2.15, TOP, "NORM")
    toggle(2.6, 2.15, TOP, "FULL")
    led(0, 0.85, TOP)
    name_text(-0.85, 1.45, TOP)
    arrows(-4.4, 4.4, -1.9, TOP)
    footswitch(0, -3.9, TOP)
    nine_volt(3.9, -5.3, TOP)
    if cfg.get("frame"):
        frame_line(W, D, TOP)
    jacks(W, -0.5, 2.3)

elif form == "compact":
    W, D, TOP = 6.2, 11.0, 3.8
    enclosure(W, D, TOP, 0.5)
    left, right = ("GAIN IN", "OUTPUT") if FAMILY == "boost" else ("OUTPUT", "GAIN IN")
    knob("left", -1.55, 3.35, 0.78, 1.1, 0.6, TOP, left)
    knob("right", 1.55, 3.35, 0.78, 1.1, 0.45, TOP, right)
    knob("mix", 0, 3.35, 0.36, 0.8, 1.0, TOP, "MIX")
    lcd(0, 1.7, TOP, w=1.8, h=0.68)
    led(0, 0.55, TOP)
    name_text(-0.8, 0.78 if len(cfg["name"]) > 7 else 0.9, TOP)
    arrows(-2.45, 2.45, -1.75, TOP)
    footswitch(0, -3.3, TOP)
    nine_volt(2.1, -4.7, TOP)
    jacks(W, 0.2, 2.1)

# ================================================================ ground, lights, camera
bpy.ops.mesh.primitive_plane_add(size=200, location=(0, 0, 0))
ground = bpy.context.active_object
ground.is_shadow_catcher = True
assign(ground, principled("ground", (0.8, 0.8, 0.8), rough=0.8))

world = bpy.data.worlds.new("world")
scene.world = world
world.use_nodes = True
wn = world.node_tree
bg = wn.nodes["Background"]
hdri = os.path.join(bpy.utils.system_resource("DATAFILES"), "studiolights", "world", "interior.exr")
if os.path.exists(hdri):
    env = wn.nodes.new("ShaderNodeTexEnvironment")
    env.image = bpy.data.images.load(hdri)
    mapping = wn.nodes.new("ShaderNodeMapping")
    coord = wn.nodes.new("ShaderNodeTexCoord")
    wn.links.new(coord.outputs["Generated"], mapping.inputs["Vector"])
    mapping.inputs["Rotation"].default_value = (0, 0, math.radians(WORLD_ROT))
    wn.links.new(mapping.outputs["Vector"], env.inputs["Vector"])
    wn.links.new(env.outputs["Color"], bg.inputs["Color"])
bg.inputs["Strength"].default_value = 0.3


def area(name, loc, size, energy, color=(1, 1, 1), target=(0, 0, 2), shadow=True):
    l = bpy.data.lights.new(name, "AREA")
    l.shape = "RECTANGLE"
    l.size, l.size_y = size
    l.energy = energy
    l.color = color
    if not shadow:
        try:
            l.use_shadow = False
        except Exception:
            pass
    o = bpy.data.objects.new(name, l)
    o.location = loc
    bpy.context.collection.objects.link(o)
    o.rotation_euler = (Vector(target) - Vector(loc)).to_track_quat("-Z", "Y").to_euler()
    return o


area("key softbox", (-7, 12, 46), (30, 22), 15000, (1.0, 0.97, 0.93))
area("fill", (16, -14, 24), (18, 14), 3200, (0.93, 0.96, 1.0), shadow=False)
area("rim", (0, 30, 18), (34, 6), 5000)

cam_data = bpy.data.cameras.new("cam")
cam_data.lens = 105
cam = bpy.data.objects.new("cam", cam_data)
bpy.context.collection.objects.link(cam)
cam.location = (0, -11, 60)
cam.rotation_euler = (Vector((0, -0.4, 2.0)) - cam.location).to_track_quat("-Z", "Y").to_euler()
scene.camera = cam

scene.render.engine = "CYCLES"
prefs = bpy.context.preferences.addons["cycles"].preferences
try:
    prefs.compute_device_type = "METAL"
    prefs.get_devices()
    for dev in prefs.devices:
        dev.use = True
    scene.cycles.device = "GPU"
except Exception as exc:
    print("GPU unavailable:", exc)
scene.cycles.samples = SAMPLES
scene.cycles.use_denoising = True
scene.render.film_transparent = True
scene.render.resolution_x = 900
scene.render.resolution_y = 1300
scene.render.image_settings.file_format = "PNG"
scene.render.image_settings.color_mode = "RGBA"
try:
    scene.view_settings.view_transform = "AgX"
    scene.view_settings.look = "AgX - Medium High Contrast"
except TypeError:
    pass
finish(scene, OUT, FAMILY, SAMPLES)
