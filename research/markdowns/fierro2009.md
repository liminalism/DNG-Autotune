<!-- Converted from fierro2009.pdf — 6 pages -->

## Page 1

### An Automatic Color Correction Method Inspired
### By The Retinex And Opponent Colors Theories
School of Electrical Engineering and Computer Science Kyungpook National University 1370 Sankyuk-Dong, Buk-Gu, Taegu 702-701, Republic of Korea. Telephone: +82 (0)53-950-5535, Fax: +82 (0)53-957-1194 Email: fierro,hogus,yha@ee.knu.ac.kr
of recovering radiance and reflectance. Unfortunately this is an ill-posed and heavily under-constrained problem [3] and it is I. INTRODUCTION the authors’ humble opinion that, without proper radiometric Color correction indicates the process of changing the information about the scene, this kind of approach should be colors of a digital image in order to reach given objectives. avoided. Most commonly color correction is used in digital imaging to For an exhaustive discussion of the well established theories imitate the human capability known as color constancy (also and methods behind computational color constancy and digital known as chromatic adaptation). Another common field of color imaging we invite the reader to refer to the excellent application is that of image enhancement. works by Sharma and Trussell [4], Ebner [1] and Fairchild Color constancy, named white balance in photography, is [5]. Here we will limit ourselves to the indication of the fields the ability of human beings to recognize an object color and theories that have seen recent contributions to the topic. disregarding (color of) the light used to illuminate the object We can start our brief discussion on the state of the art [1]. Such ability is not perfect, due to the limitations of the by mentioning the white-patch and grey-world assumptions, human sensorial and neural systems (see e.g. the phenomenon the former providing a solid grounding to the Retinex theory: of metamerism), yet it is very powerfull and allows the many algorithms have been developed based on these assump-human being to adapt to a plethora of different viewing tions even in recent years [6], [7], [8]. conditions. Human color constancy exhibits time and space Gamut-mapping (gamut-constraint) methods also prolifer-variant behaviour that make a computational version very hard ated after a first proposal by Forsyth [9]. to realize. The field of neural networks has also seen a healthy and Opposed to human color constancy, there is machine color steady production of works, quite often more strictly related constancy (or computational color constancy): i.e. color cor- to the biological side of the human visual system than other rection performed by some kind of computational device. The works [10]. idea inspiring machine color constancy is that a computer, Other works find their roots in image statistics: an effective given enough data and an adequate model, will always be approach seems to be that of determining the parameters of able to remove the illuminant color cast, mimicking the human an algorithm, or the algorithm to use, according to a pre-visual system if desired. classification of the image [11], [12]. Statistical analysis (e.g. More than ten years have passed since Funt et al. proved PCA) [13] and Bayesian inference [14] have also inspired most of the machine color constancy methods available at the many researchers. time to be “not good enough” [2] and even if a lot of progress Another recent approach in the field of multiple illuminants has been made, color constancy still remains one of the most correction interprets the color correction process as a matting prominent topics of contemporary research. problem, obtaining pleasant results but still requiring human This paper is organized as follows. We start with a brief interaction [15]. excursus on past and current literature regarding the topic In the following Section we will illustrate the problem and of color constancy (see Sec. II). In Sec. III we introduce our assumptions about it.
978-1-4244-4210-2/09/$25.00 ©2009 IEEE

---

## Page 2

III. PROBLEM FORMULATION
CIE−L∗a∗b∗, TIQ and YCC) present a magenta-cyan axis in
precisely defines the necessary transforms to and from R′G′B′.
Also, in order to perform color correction, we will be assuming that the following facts hold: 1) We can describe the image formation process through the trichromatic model
c = f (Rad × Rfl) (1)
lights) with radiance spectrum Rad
and reflectance spectra into RGB values. Note that f() encompasses all the processes needed to The flow of our algorithm is depicted in Fig. 1 and can be acquire the scene and store it on an appropriate medium roughly summarized as follows: (e.g. a RAW file from a digital camera), thus including 1) Identification of the areas of the image that will act as transformations induced by optics, medium noise, etc. lights for the color correction (see Sec. IV-A); 2) There is a suitable analysis of the RGB data that allows 2) Chromatic and spatial clustering of the light identified us to have a good estimate of luminance. It is very in the previous step (see Sec. IV-B); important to stress that we are not trying to determine 3) Computation of the per-pixel influence of each light and f(), rather we look for all those pixels that have a high a la Retinex color correction (see Sec. IV-C). luminance and that are clustered around a representative C. Since such pixels represent metameric spectra of light In step 1 we gather an estimate of luminance through the sources from a numerical point of view, we assume that computation of the luma image, then we take areas of the they should be treated in the same way, independently image that fall in the top 5% of it as reference lights. of the real scene illuminance or reflectance. We would also like to point out that the distinction between a true In step 2 we exploit chromatic information to cluster the light source (i.e. an emissive body in the scene) and a highlight regions according to their CIE − Lab ∆E00mea-reflected light spot is a matter of semantics. In the rest of sure. The clusters so formed are further checked for spatial this work we will use the word light to indicate emissive clustering by means of Gap statistics, Euclidean distance and sources as well as reflected light sources. K-means clustering.
3) The last assumption is that the Retinex model by Land In step 3 each pixel in the image that is not part of a holds (intended as in [17] and [18]), in which case we highlight area is corrected: every light in the image affects each can treat each of the RGB channels separately and the pixel by an amount depending on its chromatic and spatial maximum value on each channel represents the reference distance from the pixel itself. white for the whole image. Yet, color correction will be In the following subsections we will describe in detail each performed locally. phase of the method.

---

## Page 3

A. Highlight regions extraction and normalize it on the sum of the RGB values of the reference∑ (Von Kries) white w = Given the input image[ I (x, y)ch∈{R,G,B]}Twe determine thesum∑chWch. This is described in (6).
W W W reference white w = as: R G BS =chM (6) wch= max (Ich)1(2) wsum
(a) (b)
While running the experiments it has been noticed that, for particular images, the top 5% of the luma image does In order to obtain the estimates of the lights present in the not provide enough information and the segmentation step scene we follow the method previously used in [19], but any produces only a single class of highlight areas, resulting in other way of obtaining a good estimate for luminance may a pure Von Kries method. In order to solve this problem be used. We first gather the luma data using the formulation we suggest the repetition of the thresholding with a lower given by oRGB, expressed in (3): value each time, until more than one class is found. We are currently investigating in order to find an automatic solution to the problem.
$$ L = 0.2990 R + 0.5870 G + 0.1140 B; (3) $$
B. Chromatic and spatial clustering We then select the set of raster scan indices of those pixels in the top 5% of the normalized luma image as per (4). Since it is important to to take into account chromatic and { } spatial relationships that might tie different highlight regions L to each other, we perform a two-tier clustering devised to
$$ T = idx (p) p $$
and p > 0.95 (4)
max(L) leverage on such relationships.
We first cluster the regions in   r  
Next, we create another RGB image   M  
values of “top luma” pixels. Mathematically this is described
M(i) = (5) (this will yield a maximum intra-cluster difference of 1).
DM(k,b)mentioned in the pseudo-code. We first suppose to In order to distinguish the reference white from other lights, have clustered the data into clusters C1, ..., Ck, denoting with we compute the sum of M over the three chromatic channels, Crthe indices of observations in cluster r and with nrthe

---

## Page 4

extremes of the hue wheel). for i = 1 to R∆do
for k = 1 to < r∆(i) > do Section IV) on a non-light pixel to be equal to that of a Compute dispersion measure DMk light areas on its own pixels. for b = 1 to B do Ref(b, k) ← random {x(b,k), y(b,k)} These assumptions will be thoroughly explained in the rest of Compute dispersion measure DM∗(k,b)this section. Still, let us first define some variables for clarity:
lsdend forGap(←(k)B1k←)∑←[Bb=1B1B1∑log∑√BbBb=1(DM=1(log(DMlog(DM∗(k,b) ∗(∗(k,bk,b))−−log(DMl)2]1/2k) height(we will be considering a pixel to be correctedlinear index of the pixel in the image (index in the set of highlight clusters obtained after Fig. 4.distances:1) Light influence with respect to chromatic and spatialI)}), and an highlight areaWe define a first parameter,hji, where∈ {αi,j1, to be dependent,p2i, ...,, wherej is the linearwidth(i is theI)·
s(k) ← sd(k) · 1 + on the Euclidean distance between the piand the centroid of
h . Let ∆ if k > 1 thenj euclidbe the Euclidean distance, then if Gap(k − 1) ≥ Gap(k) + s(k) then ∆ euclid(pi, hj)
end if
Guess(i) ← k − 1 α(i,j)= 1 − (9) diag
| end if | where diag is the diagonal of the image.. |
|---|---|
| end for | Similarly to α we define a second parameter, β i,j , to be |
| end for | dependent on the distance between the hue value of p i and the |
average hue value of
absolute hue difference (
r hue∈ [0, 180]), then: pairwise distances as ∆hue(pi, hj)
$$ ∑ β(i,j)= 1 − (10) $$
i,i ∈Cr2) Correction factor: Given the definitions of α and β, we define the correction factor for a given pair of indices {i, j} then the dispersion measure DMkis computed as
$$ ∑k1 $$
r=1
something like what is shown in in Fig. 5 (b).
as:
cf
$$ = α $$
$$ · β $$
(i,j)= 1 if and only if the distance between piand hj experiments we set B = 25, a number high enough to satisfy is null and the hue difference is also null (true only for almost any need, but B should really be determined based on pixels which are part of the highlight area). the confidence level desired for the application. 3) Unitary light action: Any pixel part of an highlight region will be corrected to its own local white (the region maximum on each of the color channels). From a mathematical
j=1
its density was decreasing linearly with respect to the distance from the center. We want (12) to hold even for pixels that are not part of a • According to Hering’s theory no color looks red and highlight region: the total correcting action of the lights on a green at the same time, nor can it look yellow and blue given pixel must be equal to 1.

---

## Page 5

4) Per pixel correction: In the end, given pixel piand regions rgap, we compute the correction factor for each region as per (11). This gives us a vector of coefficients cf = cf , ..., cf
i,1 i,Rgap.
Since the pixel is given, we will omit the
following equations for ease of understanding.
the following way:
˜
cf(j)=Rj=1gapcf(j)(13)
We also build the matrix of light colors (white points) as indicated next:
max(h ) max(h ) max(h
(1,R) (1,G) (1,B))
.
max(h ) max(h ) max(h
(J,R) (J,G) (J,B))
(14)
where J = Rgap(for compactness reasons) and the maximum AVERAGE ORGBTABLE IDIFFERENCES. THE
Y C
| member pixels. |  | Y | C yb | C rg |
|---|---|---|---|---|
| The final per-pixel correction is performed according to the | Non corrected | -26.68 | -0.0808 | 0.009 |
|  | C yb C rg only | -29.56 | 0.0201 | 0.0202 |
member pixels. yb Crg
p(ch) p˜(ch)=Rj=1gapWP(j,ch)cf˜(j)(15) the exposure is far from ideal. The absence of multiple light
colors (the final skin tone is blue-ish), although the absolute average chromatic distance is reduced. The method performs V. EXPERIMENTAL SETUP AND RESULTS marginally better than its simplified version: the computational cost is much higher, but we believe that in more complex Since this work stemmed from research on skin tone color correction, even though it turned out into a general method, the experiments were run on images extracted from the “AR Face Database” [21]: this database has the interesting property of reproposing the same subjects under different viewing conditions (lighting, occluding elements, etc). We chose two images per subject, one with neutral lighting and one with a distinct non-uniform color cast. We then applied our color correction method to the image with the color cast, and verified the chromatic difference with respect to the neutrally illuminated image. On a sidenote we have
situations the extra processing will lead to significantly better results. Finally it is important to note that, being the correction inspired by Retinex, this method will always increase the light-ness of the pixel under examination (although not dramatically since we limit the possible white reference to already luminous parts of the image) and that repeatability is not granted, due to the use of a statistical measure for clustering.
VI. CONCLUSIONS AND FUTURE WORK
to mention that the database only offers left, right or “left As far as the authors’ knowledge is concerned this is the and right” yellowish additional illumination and in such cases first time that an automatic method for color correction taking

---

## Page 6

# into account both Land’s Retinex and Hering’s opponent color
[6] E. Provenzi, C. Gatta, M. Fierro, and A. Rizzi, “A spatially variant theories is devised. white-patch and gray-world method for color image enhancement driven by local contrast,” Pattern Analysis and Machine Intelligence, IEEE
# The proposed method is a generic color correction algorithm
Transactions on, vol. 30, no. 10, pp. 1757–1770, Oct. 2008.
# theoretically able to deal with multiple light sources, and it is
[7] A. Rizzi and C. Gatta, “From retinex to automatic color equalization: is-based on the assumption that any area of an image that has a sues in developing a new algorithm for unsupervised color equalization,” Journal of Electronic Imaging, vol. 13, no. 1, pp. 75–84, 2004.
# high enough estimate of luminance should be considered as a
[8] J. van de Weijer, T. Gevers, and A. Gijsenij, “Edge-based color con-
# light. Any light that differs from the image reference white,
stancy,” Image Processing, IEEE Transactions on, vol. 16, no. 9, pp. the global white point, influences all other pixels according to 2207–2214, Sept. 2007. [9] A. Gijsenij, T. Gevers, and J. Weijer, “Color Constancy by Derivative-
# both Retinex and the opponent colors theories.
based Gamut Mapping,” in Proceedings of the First International
# Tests have been conducted in the field of skin color correc-
Workshop on Photometric Analysis For Computer Vision - PACV 2007, tion, albeit with just one non-uniform light. The experiments Peter Belhumeur, Katsushi Ikeuchi, Emmanuel Prados, Stefano Soatto, and Peter Sturm, Eds. Rio de Janeiro Brazil: INRIA, 2007, p. 8 p.
# show how the rendition of skin color improves notably after
[Online]. Available: http://hal.inria.fr/inria-00264813/en/
# the proposed method is applied, but also indicate how the [10] J. Lau and B. Shi, “Improved illumination invariance using a color
method needs further research and tuning to achieve better edge representation based on double opponent neurons,” in Neural Networks, 2008. IJCNN 2008. (IEEE World Congress on Computational
# results.
Intelligence). IEEE International Joint Conference on, June 2008, pp.
# Further research on the topic will regard the estimation of
2735–2741. the luma threshold in the case the method reverts to Von [11] A. Gijsenij and T. Gevers, “Color constancy using natural image statistics,” in Computer Vision and Pattern Recognition, 2007. CVPR
# Kries processing, the possibility of merging the two clustering
’07. IEEE Conference on, June 2007, pp. 1–8. steps, estimation of a proper value for the parameter B, and [12] S. Bianco, G. Ciocca, C. Cusano, and R. Schettini, “Improving color evaluation of the performance of different formulations of the constancy using indoor/outdoor image classification,” Image Processing, IEEE Transactions on, vol. 17, no. 12, pp. 2381–2392, Dec. 2008.
# correction factor.
[13] C.-H. Hsu, Z.-W. Chen, and C.-C. Chiang, “Region-based color cor-Further testing will be carried out on more challenging rection of images,” in Information Technology and Applications, 2005. databases, especially to test the method ability to correct ICITA 2005. Third International Conference on, vol. 1, July 2005, pp. 710–715 vol.2.
# multiple illuminants, and the results will be compared to other [14] P. Gehler, C. Rother, A. Blake, T. Minka, and T. Sharp, “Bayesian color
state of the art algorithms. constancy revisited,” in Computer Vision and Pattern Recognition, 2008. CVPR 2008. IEEE Conference on, June 2008, pp. 1–8. ACKNOWLEDGMENT [15] E. Hsu, T. Mertens, S. Paris, S. Avidan, and F. Durand, “Light mixture estimation for spatially varying white balance,” ACM Trans. Graph.,
# This research is supported by Ministry of Culture, Sports
vol. 27, no. 3, pp. 70–77, 2008. and Tourism (MCST) and Korea Culture Content Agency [16] M. Bratkova, S. Boulos, and P. Shirley, “orgb: A practical opponent color (KOCCA) in the Culture Technology (CT) Research & De- space for computer graphics,” Computer Graphics and Applications, IEEE, vol. 29, no. 1, pp. 42–55, Jan.-Feb. 2009.
# velopment Program 2009.
[17] E. Land and J. J. McCann, “Lightness and retinex theory,” Journal of Optical Society of America A, vol. 61, pp. 1–11, 1971.
# REFERENCES
Available: http://cobweb.ecn.purdue.edu/∼aleix/aleix face DB.html

### Extracted Citations (JSON)
```json
[
  {
    "id": "18",
    "text": "E. Provenzi, M. Fierro, A. Rizzi, L. De Carli, D. Gadia, and D. Marini, “Random spray retinex: A new retinex implementation to investigate the"
  },
  {
    "id": "1",
    "text": "M. Ebner, Color Constancy. Wiley & Sons, 2007. local properties of the model,” Image Processing, IEEE Transactions on,"
  },
  {
    "id": "2",
    "text": "B. Funt, K. Barnard, and L. Martin, “Is machine colour constancy vol. 16, no. 1, pp. 162–171, Jan. 2007. good enough?” in In Proceedings of the 5th European Conference on"
  },
  {
    "id": "19",
    "text": "R. lien Hsu, M. Abdel-mottaleb, and A. K. Jain, “Face detection in Computer Vision. Springer, 1998, pp. 445–459. color images,” IEEE Transactions on Pattern Analysis and Machine"
  },
  {
    "id": "3",
    "text": "A. C. Hurlbert, “Formal connections between lightness algorithms,” Intelligence, vol. 24, pp. 696–706, 2002. Journal of the Optical Society of America A, vol. 3, pp. 1684–1693,"
  },
  {
    "id": "20",
    "text": "R. Tibshirani, G. Walther, and T. Hastie, “Estimating the number of 1986. clusters in a data set via the gap statistic,” Journal of the Royal"
  },
  {
    "id": "4",
    "text": "G. Sharma and H. J. Trussell, “Digital color imaging,” IEEE Transac- Statistical Society. Series B (Statistical Methodology), vol. 63, no. 2, pp. tions on Image Processing, vol. 6, pp. 901–932, 1997. 411–423, 2001. [Online]. Available: http://www.jstor.org/stable/2680607"
  },
  {
    "id": "5",
    "text": "M. D. Fairchlid, Color Appearance Model, 2nd ed. Wiley & Sons,"
  },
  {
    "id": "21",
    "text": "A. Martinez and R. Benavente. (1998) The ar face database. [Online]. 2005. Available: http://cobweb.ecn.purdue.edu/∼aleix/aleix face DB.html"
  }
]
```