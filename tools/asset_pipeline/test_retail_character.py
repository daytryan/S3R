import unittest
from .retail_character import SkeletonSet


def bone(index, name, parent=-1):
    return {"index": index, "name": name, "parent": parent}


class ParentNames(unittest.TestCase):
    def test_a_parentless_eye_rig_does_not_hide_the_head_parent(self):
        # faces.md 4A: the Organ (eyes) rx2 sorts before the Rostral (head) rx2 and lists the face leaves parentless.
        rig = SkeletonSet.__new__(SkeletonSet)
        rig._parent_names = None
        rig.models = [
            ("Organ", "0x0e1a.rx2", [bone(0, "LeftEye"), bone(1, "RightEye")], []),
            ("Rostral", "0x0ee8.rx2", [bone(0, "Head"), bone(1, "Face", 0), bone(2, "LeftEye", 1), bone(3, "RightEye", 1)], []),
        ]
        names = rig._rx2_parent_names()
        self.assertEqual(names["LEFTEYE"], "FACE")
        self.assertEqual(names["RIGHTEYE"], "FACE")
        self.assertEqual(names["FACE"], "HEAD")
        self.assertIsNone(names["HEAD"])

    def test_the_first_real_parent_wins(self):
        rig = SkeletonSet.__new__(SkeletonSet)
        rig._parent_names = None
        rig.models = [
            ("A", "a.rx2", [bone(0, "Head"), bone(1, "Jaw", 0)], []),
            ("B", "b.rx2", [bone(0, "Face"), bone(1, "Jaw", 0)], []),
        ]
        self.assertEqual(rig._rx2_parent_names()["JAW"], "HEAD")


if __name__ == "__main__":
    unittest.main()
