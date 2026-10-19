"""Render Rust CPU-skinned gameplay captures at fixed scale; no game launch."""
from pathlib import Path
import struct
import argparse
import numpy as np
from PIL import Image
from profile_preview import plt, draw


def read_poses(path):
    raw = path.read_bytes(); offset = 8
    assert raw[:8] == b"JKPOSES2"
    cases, frames, vertices, triangles = struct.unpack_from("<4I", raw, offset); offset += 16
    ids = np.frombuffer(raw, "<u4", frames, offset).copy(); offset += frames * 4
    faces = np.frombuffer(raw, "<u4", triangles*3, offset).reshape(-1,3).copy(); offset += triangles * 12
    result = {}
    for _ in range(cases):
        n, = struct.unpack_from("<I", raw, offset); offset += 4
        name = raw[offset:offset+n].decode(); offset += n
        controls = struct.unpack_from("<4f", raw, offset); offset += 16
        points = np.frombuffer(raw, "<f4", frames*vertices*3, offset).reshape(frames,vertices,3).copy(); offset += frames*vertices*12
        result[name] = points
    assert offset == len(raw)
    return ids, faces, result


def render(model, output, gif=False):
    ids, faces, poses = read_poses(output / f"{model}_poses.bin")
    # Undo the OpenJK root coordinate turn for anatomical front/side views.
    # One neutral floor offset per model, shared across every case and frame.
    floor = poses["neutral"][0,:,2].min()
    for case, points in poses.items():
        points = points[:,:,[1,0,2]] * np.array([1.,-1.,1.])
        points[:,:,2] -= floor
        poses[case] = points
    limits = [(-22,22),(-6,70)]
    for cases,suffix in [
        (["weight_low", "neutral", "weight_high", "bulky", "child", "elder"],"animated_review"),
        (["muscle_low", "muscle_high", "proportions_low", "proportions_high", "child_heavy", "child_bulky"],"stress_review"),
    ]:
        render_sheet(model,output,ids,faces,poses,limits,cases,suffix)
    if gif:
        images=[]
        fig,axes=plt.subplots(1,4,figsize=(12,5),facecolor="#111b28")
        for fi in range(2,28,2):
            for ax,case in zip(axes,["neutral","weight_high","bulky","child"]):
                ax.clear(); draw(ax,poses[case][fi],faces,limits,"front"); ax.set_title(case,color="white")
            fig.suptitle(f"{model} / actual WALK1 frames / fixed camera scale",color="white")
            fig.canvas.draw()
            images.append(Image.fromarray(np.asarray(fig.canvas.buffer_rgba())[:,:,:3]).convert("P",palette=Image.Palette.ADAPTIVE))
        images[0].save(output/f"{model}_walk.gif",save_all=True,append_images=images[1:],duration=100,loop=0)
        plt.close(fig)


def render_sheet(model,output,ids,faces,poses,limits,cases,suffix):
    fig, axes = plt.subplots(3,12,figsize=(24,14),facecolor="#111b28")
    for row,(fi,clip) in enumerate([(0,"Stand"),(11,"Walk"),(31,"Crouch")]):
        for ci,case in enumerate(cases):
            for vi,view in enumerate(["front","side"]):
                ax=axes[row,ci*2+vi]
                draw(ax,poses[case][fi],faces,limits,view)
                ax.set_title(f"{clip} / {case}\n{view} / frame {ids[fi]}",fontsize=8,color="white")
    fig.suptitle(f"{model} / Rust Ghoul2 skinner + client body binding / fixed camera scale",color="white")
    fig.tight_layout(rect=(0,0,1,.96)); fig.savefig(output/f"{model}_{suffix}.png",dpi=100); plt.close(fig)


if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--model",default="jedi_hm");parser.add_argument("--gif",action="store_true");args=parser.parse_args()
    render(args.model,Path(__file__).parent/"output",args.gif)
